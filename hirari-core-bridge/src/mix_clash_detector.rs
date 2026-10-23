/// Analyze two non-negative magnitude spectra and classify their strongest
/// overlap. Advice values match `MixClashDetector::AdviceCode` in C++.
#[no_mangle]
pub unsafe extern "C" fn hirari_mix_clash_detect(
    spectrum_a: *const f32,
    spectrum_b: *const f32,
    size: usize,
    sample_rate: f64,
    masking_index: *mut f32,
    center_frequency: *mut f32,
    advice: *mut u8,
) -> bool {
    if spectrum_a.is_null() || spectrum_b.is_null() || size == 0
        || !sample_rate.is_finite() || sample_rate <= 0.0
        || masking_index.is_null() || center_frequency.is_null() || advice.is_null() {
        return false;
    }
    let a = std::slice::from_raw_parts(spectrum_a, size);
    let b = std::slice::from_raw_parts(spectrum_b, size);
    let (mut total_a, mut total_b, mut overlap, mut peak, mut peak_bin) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0usize);
    for (i, (&raw_a, &raw_b)) in a.iter().zip(b).enumerate() {
        let av = if raw_a.is_finite() { raw_a.max(0.0) } else { 0.0 };
        let bv = if raw_b.is_finite() { raw_b.max(0.0) } else { 0.0 };
        total_a += av;
        total_b += bv;
        let shared = av.min(bv);
        overlap += shared;
        if shared > peak { peak = shared; peak_bin = i; }
    }
    let masking = (overlap / total_a.min(total_b).max(1.0e-6)).clamp(0.0, 1.0);
    let center = peak_bin as f32 * sample_rate as f32 / size as f32;
    *masking_index = masking;
    *center_frequency = center;
    *advice = if masking > 0.75 { 1 } else if masking > 0.5 { 2 } else { 3 };
    true
}
