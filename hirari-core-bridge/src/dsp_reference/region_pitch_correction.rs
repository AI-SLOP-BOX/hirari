//! Frozen pitch-correction delay schedule used by the region pitch tests.

use super::region_resampler::hirari_region_read_warped_reference;

#[no_mangle]
pub unsafe extern "C" fn region_pitch_correction_reference_delays(
    sample_rate: f64,
    reference_pitch_cents: f64,
    output: *mut f64,
) -> bool {
    if output.is_null() || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    let reference_hz = if reference_pitch_cents.is_finite() && reference_pitch_cents > 0.0 {
        440.0 * f64::exp2(((reference_pitch_cents - 6900.0) / 1200.0).clamp(-5.0, 5.0))
    } else {
        140.0
    };
    let reference_period = sample_rate / reference_hz.clamp(20.0, 2000.0);
    let minimum_delay = (2.0 * reference_period).max(256.0).clamp(256.0, 2048.0);
    let maximum_delay = (8.0 * reference_period)
        .max(minimum_delay + 256.0)
        .clamp(minimum_delay + 256.0, 7000.0);
    *output = minimum_delay;
    *output.add(1) = maximum_delay - minimum_delay;
    *output.add(2) = reference_hz;
    true
}

#[no_mangle]
pub unsafe extern "C" fn region_pitch_correction_reference_frame(
    source_left: *const f32,
    source_right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    warped_position: f64,
    local_source_rate: f64,
    effective_pitch_ratio: f64,
    base_pitch_ratio: f64,
    loop_relative: u64,
    note_correction_seconds: f64,
    sample_rate: f64,
    minimum_delay: f64,
    delay_range: f64,
    reverse: u8,
    kernel: *const f32,
    output: *mut f32,
) -> bool {
    if source_left.is_null()
        || source_right.is_null()
        || kernel.is_null()
        || output.is_null()
        || delay_range <= 0.0
    {
        return false;
    }
    let delay_travel = local_source_rate
        * ((1.0 - base_pitch_ratio) * loop_relative as f64
            - base_pitch_ratio * note_correction_seconds * sample_rate);
    let mut phase_a = (delay_travel / delay_range) % 1.0;
    if phase_a < 0.0 {
        phase_a += 1.0;
    }
    let mut phase_b = phase_a + 0.5;
    if phase_b >= 1.0 {
        phase_b -= 1.0;
    }
    let delay_a = minimum_delay + phase_a * delay_range;
    let delay_b = minimum_delay + phase_b * delay_range;
    let weight_a = (1.0 - 2.0 * (phase_a - 0.5).abs()) as f32;
    let weight_b = (1.0 - 2.0 * (phase_b - 0.5).abs()) as f32;
    let step = local_source_rate * effective_pitch_ratio;
    let mut left_a = [0.0f32; 2];
    let mut left_b = [0.0f32; 2];
    let mut right_a = [0.0f32; 2];
    let mut right_b = [0.0f32; 2];
    hirari_region_read_warped_reference(
        source_left,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_a,
        reverse,
        1,
        step,
        kernel,
        left_a.as_mut_ptr(),
    );
    hirari_region_read_warped_reference(
        source_left,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_b,
        reverse,
        1,
        step,
        kernel,
        left_b.as_mut_ptr(),
    );
    hirari_region_read_warped_reference(
        source_right,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_a,
        reverse,
        1,
        step,
        kernel,
        right_a.as_mut_ptr(),
    );
    hirari_region_read_warped_reference(
        source_right,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_b,
        reverse,
        1,
        step,
        kernel,
        right_b.as_mut_ptr(),
    );
    *output = left_a[0] * weight_a + left_b[0] * weight_b;
    *output.add(1) = left_a[1] * weight_a + left_b[1] * weight_b;
    *output.add(2) = right_a[0] * weight_a + right_b[0] * weight_b;
    *output.add(3) = right_a[1] * weight_a + right_b[1] * weight_b;
    true
}
