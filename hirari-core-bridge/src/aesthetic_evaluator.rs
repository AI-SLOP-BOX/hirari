#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AestheticFeaturesFfi {
    pub spectral_balance: f32,
    pub dynamic_complexity: f32,
    pub transient_clarity: f32,
    pub stereo_width: f32,
    pub phase_coherence: f32,
}

/// Computes the signal statistics used by the native mix quality analyzer.
#[no_mangle]
pub unsafe extern "C" fn hirari_aesthetic_analyze(
    left: *const f32,
    right: *const f32,
    samples: u32,
    output: *mut AestheticFeaturesFfi,
) {
    if output.is_null() {
        return;
    }
    let result = if left.is_null() || right.is_null() || samples == 0 {
        AestheticFeaturesFfi {
            spectral_balance: 0.5,
            dynamic_complexity: 0.5,
            transient_clarity: 0.5,
            stereo_width: 0.0,
            phase_coherence: 1.0,
        }
    } else {
        let left = unsafe { std::slice::from_raw_parts(left, samples as usize) };
        let right = unsafe { std::slice::from_raw_parts(right, samples as usize) };
        let (mut sum_sq_l, mut sum_sq_r, mut peak_l, mut peak_r, mut dot) =
            (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (&raw_l, &raw_r) in left.iter().zip(right) {
            let l = if raw_l.is_finite() {
                raw_l.clamp(-4.0, 4.0)
            } else {
                0.0
            };
            let r = if raw_r.is_finite() {
                raw_r.clamp(-4.0, 4.0)
            } else {
                0.0
            };
            sum_sq_l += l * l;
            sum_sq_r += r * r;
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
            dot += l * r;
        }
        let rms_l = (sum_sq_l / samples as f32).sqrt();
        let rms_r = (sum_sq_r / samples as f32).sqrt();
        let average_rms = (rms_l + rms_r) * 0.5;
        let average_peak = (peak_l + peak_r) * 0.5;
        let denominator = (sum_sq_l * sum_sq_r).sqrt() + 1.0e-6;
        let dynamic = ((average_peak / (average_rms + 1.0e-6)) / 10.0).clamp(0.0, 1.0);
        AestheticFeaturesFfi {
            spectral_balance: ((rms_l - rms_r).abs() / (average_rms + 1.0e-6)).clamp(0.0, 1.0),
            dynamic_complexity: dynamic,
            transient_clarity: (dynamic * 1.5).clamp(0.0, 1.0),
            stereo_width: (1.0 - dot.abs() / denominator).clamp(0.0, 1.0),
            phase_coherence: (dot / denominator).clamp(-1.0, 1.0),
        }
    };
    unsafe { output.write(result) };
}
