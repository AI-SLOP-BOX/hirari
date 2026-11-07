use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct MixMetrics {
    pub sample_count: usize,
    pub rms_db: f32,
    pub peak_db: f32,
    pub true_peak_db: f32,
    pub lufs: f32,
    pub left_lufs: f32,
    pub right_lufs: f32,
    pub crest_db: f32,
    pub correlation: f32,
    pub mono_rms_db: f32,
    pub mono_delta_db: f32,
    pub mono_compatible: bool,
    pub spectral_centroid_hz: f32,
    pub spectral_peak_hz: f32,
}

fn db(value: f64) -> f32 {
    (20.0 * value.max(1.0e-12).log10()) as f32
}

pub fn analyze(left: &[f32], right: &[f32]) -> MixMetrics {
    analyze_with_sample_rate(left, right, 48_000.0)
}

pub fn analyze_with_sample_rate(left: &[f32], right: &[f32], sample_rate: f32) -> MixMetrics {
    let count = left.len().min(right.len());
    if count == 0 {
        return MixMetrics {
            sample_count: 0,
            rms_db: -240.0,
            peak_db: -240.0,
            true_peak_db: -240.0,
            lufs: -240.0,
            left_lufs: -240.0,
            right_lufs: -240.0,
            crest_db: 0.0,
            correlation: 0.0,
            mono_rms_db: -240.0,
            mono_delta_db: 0.0,
            mono_compatible: true,
            spectral_centroid_hz: 0.0,
            spectral_peak_hz: 0.0,
        };
    }
    let mut sum = 0.0f64;
    let mut left_sum = 0.0f64;
    let mut right_sum = 0.0f64;
    let mut mono_sum = 0.0f64;
    let mut peak = 0.0f64;
    let mut cross = 0.0f64;
    let mut left_energy = 0.0f64;
    let mut right_energy = 0.0f64;
    let mut true_peak = 0.0f64;
    let mut previous_l = 0.0f64;
    let mut previous_r = 0.0f64;
    for (index, (&l, &r)) in left.iter().zip(right).take(count).enumerate() {
        let l = if l.is_finite() { l as f64 } else { 0.0 };
        let r = if r.is_finite() { r as f64 } else { 0.0 };
        sum += (l * l + r * r) * 0.5;
        left_sum += l * l;
        right_sum += r * r;
        let mono = (l + r) * 0.5;
        mono_sum += mono * mono;
        peak = peak.max(l.abs()).max(r.abs());
        cross += l * r;
        left_energy += l * l;
        right_energy += r * r;
        true_peak = true_peak.max(l.abs()).max(r.abs());
        if index > 0 {
            for fraction in 1..4 {
                let t = fraction as f64 * 0.25;
                true_peak = true_peak
                    .max((previous_l + (l - previous_l) * t).abs())
                    .max((previous_r + (r - previous_r) * t).abs());
            }
        }
        previous_l = l;
        previous_r = r;
    }
    let rms = (sum / count as f64).sqrt();
    let mono_rms = (mono_sum / count as f64).sqrt();
    let rms_db = db(rms);
    let peak_db = db(peak);
    let fallback_left = || (-0.691 + 10.0 * (left_sum / count as f64).max(1.0e-24).log10()) as f32;
    let fallback_right =
        || (-0.691 + 10.0 * (right_sum / count as f64).max(1.0e-24).log10()) as f32;
    let (left_lufs, right_lufs) = if sample_rate.is_finite()
        && (8_000.0..=192_000.0).contains(&sample_rate)
        && count >= ((sample_rate * 0.4).round() as usize).max(1)
    {
        let window = ((sample_rate * 0.4).round() as usize).max(1);
        let measure = |channel: &[f32]| {
            // Duplicate a mono channel into L/R so the stereo meter's
            // weighting does not introduce an artificial -3 dB offset.
            serde_json::from_str::<serde_json::Value>(
                &crate::stable_api::CoreApiV1::waveform_integrated_lufs_json(
                    channel,
                    channel,
                    sample_rate,
                    window,
                ),
            )
            .ok()
            .and_then(|value| {
                value
                    .get("integrated_lufs_estimate")
                    .and_then(|v| v.as_f64())
            })
            .filter(|value| value.is_finite())
            .map(|value| value as f32)
        };
        (
            measure(left).unwrap_or_else(fallback_left),
            measure(right).unwrap_or_else(fallback_right),
        )
    } else {
        (fallback_left(), fallback_right())
    };
    // Keep MixConsole's integrated loudness aligned with the waveform editor:
    // use the offline BS.1770 K-weighted stereo meter when a valid sample rate
    // is available, and retain the bounded RMS fallback for malformed rates.
    let lufs = if sample_rate.is_finite()
        && (8_000.0..=192_000.0).contains(&sample_rate)
        && count >= ((sample_rate * 0.4).round() as usize).max(1)
    {
        let window = ((sample_rate * 0.4).round() as usize).max(1);
        serde_json::from_str::<serde_json::Value>(
            &crate::stable_api::CoreApiV1::waveform_integrated_lufs_json(
                left,
                right,
                sample_rate,
                window,
            ),
        )
        .ok()
        .and_then(|value| {
            value
                .get("integrated_lufs_estimate")
                .and_then(|v| v.as_f64())
        })
        .filter(|value| value.is_finite())
        .map(|value| value as f32)
        .unwrap_or_else(|| (-0.691 + 10.0 * (sum / count as f64).max(1.0e-24).log10()) as f32)
    } else {
        (-0.691 + 10.0 * (sum / count as f64).max(1.0e-24).log10()) as f32
    };
    let (centroid, peak_frequency) = spectral_summary(left, right, sample_rate);
    MixMetrics {
        sample_count: count,
        rms_db,
        peak_db,
        true_peak_db: db(true_peak),
        lufs,
        left_lufs,
        right_lufs,
        crest_db: (peak_db - rms_db).max(0.0),
        correlation: (cross / (left_energy * right_energy).sqrt().max(1.0e-12)).clamp(-1.0, 1.0)
            as f32,
        mono_rms_db: db(mono_rms),
        mono_delta_db: db(mono_rms) - rms_db,
        mono_compatible: mono_rms + 6.0e-12 >= rms * 0.25,
        spectral_centroid_hz: centroid,
        spectral_peak_hz: peak_frequency,
    }
}

fn spectral_summary(left: &[f32], right: &[f32], sample_rate: f32) -> (f32, f32) {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return (0.0, 0.0);
    }
    let n = left.len().min(right.len()).min(1024);
    if n < 4 {
        return (0.0, 0.0);
    }
    let mut weighted = 0.0f64;
    let mut total = 0.0f64;
    let mut peak = 0.0f64;
    let mut peak_bin = 0usize;
    for bin in 1..=(n / 2) {
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for i in 0..n {
            let sample = ((left[i] as f64 + right[i] as f64) * 0.5).clamp(-1.0, 1.0);
            let phase = 2.0 * std::f64::consts::PI * bin as f64 * i as f64 / n as f64;
            re += sample * phase.cos();
            im -= sample * phase.sin();
        }
        let magnitude = (re * re + im * im).sqrt();
        let frequency = bin as f64 * sample_rate as f64 / n as f64;
        weighted += frequency * magnitude;
        total += magnitude;
        if magnitude > peak {
            peak = magnitude;
            peak_bin = bin;
        }
    }
    if total <= 1.0e-12 {
        (0.0, 0.0)
    } else {
        (
            (weighted / total) as f32,
            (peak_bin as f32 * sample_rate / n as f32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::analyze;
    #[test]
    fn reports_phase_cancellation_and_mono_loss() {
        let metrics = analyze(&[0.5; 8], &[-0.5; 8]);
        assert!((metrics.correlation + 1.0).abs() < 0.001);
        assert!(metrics.mono_delta_db < -100.0);
        assert!(!metrics.mono_compatible);
        assert!(metrics.lufs < -5.0);
    }

    #[test]
    fn reports_lufs_and_true_peak_without_clipping_the_input() {
        let metrics = analyze(&[0.9, -0.9, 0.9, -0.9], &[0.9, -0.9, 0.9, -0.9]);
        assert!(metrics.true_peak_db < 0.0);
        assert!(metrics.lufs < 0.0);
        assert_eq!(metrics.left_lufs, metrics.right_lufs);
    }
}
