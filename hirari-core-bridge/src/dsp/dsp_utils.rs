//! Scalar DSP helpers exposed to source-compatible native callers.

#[no_mangle]
pub extern "C" fn hirari_dsp_hard_clip(value: f32) -> f32 {
    if value < -1.0 {
        -1.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

#[no_mangle]
pub extern "C" fn hirari_dsp_decay_coefficient(release_ms: f32, sample_rate: f64) -> f32 {
    (-1.0_f32 / (release_ms * 0.001_f32 * sample_rate as f32)).exp()
}

#[no_mangle]
pub extern "C" fn hirari_dsp_db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0_f32)
}

#[no_mangle]
pub extern "C" fn hirari_dsp_interpolate_hermite(
    y0: f32,
    y1: f32,
    y2: f32,
    y3: f32,
    t: f32,
) -> f32 {
    let a = -0.5_f32 * y0 + 1.5_f32 * y1 - 1.5_f32 * y2 + 0.5_f32 * y3;
    let b = y0 - 2.5_f32 * y1 + 2.0_f32 * y2 - 0.5_f32 * y3;
    let c = -0.5_f32 * y0 + 0.5_f32 * y2;
    a * t * t * t + b * t * t + c * t + y1
}

#[no_mangle]
pub extern "C" fn hirari_dsp_sanitize(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[no_mangle]
pub extern "C" fn hirari_samples_to_milliseconds(samples: u64, sample_rate: f64) -> f32 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 { return 0.0; }
    (samples as f64 * 1000.0 / sample_rate) as f32
}
