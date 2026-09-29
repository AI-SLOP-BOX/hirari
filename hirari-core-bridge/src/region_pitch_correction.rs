use super::audio_note_curve::{hirari_audio_note_curve_integral_at, HirariAudioNoteCurveView};
use std::slice;

/// Computes the bounded delay window from the pitch reference used by Track.
/// Output contains minimum delay and the usable delay range in source frames.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_correction_delays(
    sample_rate: f64,
    reference_pitch_cents: f64,
    output_minimum_range_and_reference_hz: *mut f64,
) -> bool {
    if output_minimum_range_and_reference_hz.is_null()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
    {
        return false;
    }
    let output = slice::from_raw_parts_mut(output_minimum_range_and_reference_hz, 3);
    let reference_hz = if reference_pitch_cents.is_finite() && reference_pitch_cents > 0.0 {
        440.0 * (2.0f64).powf(((reference_pitch_cents - 6900.0) / 1200.0).clamp(-5.0, 5.0))
    } else {
        140.0
    };
    let reference_period = sample_rate / reference_hz.clamp(20.0, 2000.0);
    let minimum_delay = (2.0 * reference_period).max(256.0).clamp(256.0, 2048.0);
    let minimum_maximum = minimum_delay + 256.0;
    let maximum_delay = (8.0 * reference_period)
        .max(minimum_maximum)
        .clamp(minimum_maximum, 7000.0);
    output[0] = minimum_delay;
    output[1] = maximum_delay - minimum_delay;
    output[2] = reference_hz;
    true
}

/// Runs Track's two-phase delay pitch correction for one stereo frame. The
/// caller owns source buffers; Rust computes both delay taps, performs the
/// Hermite/polyphase reads, and combines values and local slopes without heap
/// allocation.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_corrected_frame(
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
    if output.is_null() {
        return false;
    }
    let output = slice::from_raw_parts_mut(output, 4);
    output.fill(0.0);
    if source_left.is_null()
        || source_right.is_null()
        || kernel.is_null()
        || ![
            warped_position,
            local_source_rate,
            effective_pitch_ratio,
            base_pitch_ratio,
            note_correction_seconds,
            sample_rate,
            minimum_delay,
            delay_range,
        ]
        .iter()
        .all(|value| value.is_finite())
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
    let resample_step = local_source_rate * effective_pitch_ratio;
    let mut left_a = [0.0f32; 2];
    let mut left_b = [0.0f32; 2];
    let mut right_a = [0.0f32; 2];
    let mut right_b = [0.0f32; 2];
    super::region_resampler::hirari_region_read_warped(
        source_left,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_a,
        reverse,
        1,
        resample_step,
        kernel,
        left_a.as_mut_ptr(),
    );
    super::region_resampler::hirari_region_read_warped(
        source_left,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_b,
        reverse,
        1,
        resample_step,
        kernel,
        left_b.as_mut_ptr(),
    );
    super::region_resampler::hirari_region_read_warped(
        source_right,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_a,
        reverse,
        1,
        resample_step,
        kernel,
        right_a.as_mut_ptr(),
    );
    super::region_resampler::hirari_region_read_warped(
        source_right,
        source_samples,
        source_offset,
        source_span,
        warped_position - delay_b,
        reverse,
        1,
        resample_step,
        kernel,
        right_b.as_mut_ptr(),
    );
    output[0] = left_a[0] * weight_a + left_b[0] * weight_b;
    output[1] = left_a[1] * weight_a + left_b[1] * weight_b;
    output[2] = right_a[0] * weight_a + right_b[0] * weight_b;
    output[3] = right_a[1] * weight_a + right_b[1] * weight_b;
    true
}

/// Computes note-relative phase correction from the selected Rust curve views
/// and renders the corrected stereo frame in the same call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_corrected_frame_with_curves(
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
    note_seconds: f64,
    sample_rate: f64,
    minimum_delay: f64,
    delay_range: f64,
    reverse: u8,
    kernel: *const f32,
    matched_curve: *const HirariAudioNoteCurveView,
    previous_curve: *const HirariAudioNoteCurveView,
    output: *mut f32,
) -> bool {
    let note_correction_seconds = if !matched_curve.is_null() {
        let curve = &*matched_curve;
        let segment_offset = (note_seconds - curve.start_seconds).max(0.0);
        let integrated = hirari_audio_note_curve_integral_at(
            curve.anchors,
            curve.anchor_count,
            curve.integral_prefix,
            curve.start_seconds,
            curve.end_seconds,
            curve.pitch_offset_cents,
            note_seconds,
        );
        curve.correction_before_seconds + integrated - segment_offset
    } else if !previous_curve.is_null() {
        let curve = &*previous_curve;
        let integrated_to_end = hirari_audio_note_curve_integral_at(
            curve.anchors,
            curve.anchor_count,
            curve.integral_prefix,
            curve.start_seconds,
            curve.end_seconds,
            curve.pitch_offset_cents,
            curve.end_seconds,
        );
        curve.correction_before_seconds + integrated_to_end
            - (curve.end_seconds - curve.start_seconds)
    } else {
        0.0
    };
    hirari_region_pitch_corrected_frame(
        source_left,
        source_right,
        source_samples,
        source_offset,
        source_span,
        warped_position,
        local_source_rate,
        effective_pitch_ratio,
        base_pitch_ratio,
        loop_relative,
        note_correction_seconds,
        sample_rate,
        minimum_delay,
        delay_range,
        reverse,
        kernel,
        output,
    )
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "region_pitch_correction_differential_tests.rs"]
mod differential_tests;
