//! Scalar DSP helpers exposed to source-compatible native callers.

#[no_mangle]
pub extern "C" fn hirari_samples_to_milliseconds(samples: u64, sample_rate: f64) -> f32 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 { return 0.0; }
    (samples as f64 * 1000.0 / sample_rate) as f32
}
