use super::audio_note_curve::{
    hirari_audio_note_curve_at, hirari_audio_note_find_segment_ranges, HirariAudioNoteCurveView,
    HirariAudioNoteSegmentRange,
};
use super::region_warp::{hirari_region_source_position_at, HirariWarpMarker};
use std::slice;

const MAX_CORRELATION_POINTS: usize = 32;
const MAX_ACTIVE_GRAINS: usize = 4;

/// Selects a WSOLA grain by normalized stereo correlation. The caller builds
/// the timeline-to-source offsets once per grain; the bounded candidate scan
/// and audio reads live in Rust and allocate nothing on the render thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_wsola_select_grain(
    left: *const f32,
    right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    expected: f64,
    reference: f64,
    overlap_span: f64,
    reference_span: f64,
    center: i64,
    last: i64,
    search_radius: i64,
    reverse: u8,
    current_offsets: *const f64,
    previous_offsets: *const f64,
    point_count: u32,
) -> f64 {
    if left.is_null()
        || right.is_null()
        || current_offsets.is_null()
        || previous_offsets.is_null()
        || source_span == 0
        || source_offset
            .checked_add(source_span)
            .is_none_or(|end| end > source_samples)
        || source_samples > isize::MAX as u64 / std::mem::size_of::<f32>() as u64
        || ![expected, reference, overlap_span, reference_span]
            .iter()
            .all(|value| value.is_finite())
        || search_radius < 0
        || point_count == 0
        || point_count as usize > MAX_CORRELATION_POINTS
    {
        return expected;
    }

    let current_offsets = slice::from_raw_parts(current_offsets, point_count as usize);
    let previous_offsets = slice::from_raw_parts(previous_offsets, point_count as usize);
    if !current_offsets
        .iter()
        .chain(previous_offsets)
        .all(|value| value.is_finite())
    {
        return expected;
    }

    let source_offset = source_offset as i64;
    let source_span = source_span as i64;
    let last = last.min(source_span - 1);
    let reversed = reverse != 0;
    let score_candidate = |candidate: i64| {
        if candidate < 0
            || candidate as f64 + overlap_span >= last as f64
            || reference < 0.0
            || reference + reference_span >= last as f64
        {
            return -2.0;
        }

        let mut dot_left = 0.0;
        let mut energy_a_left = 0.0;
        let mut energy_b_left = 0.0;
        let mut dot_right = 0.0;
        let mut energy_a_right = 0.0;
        let mut energy_b_right = 0.0;
        for (&current_offset, &previous_offset) in current_offsets.iter().zip(previous_offsets) {
            let a_relative = (candidate as f64 + current_offset).round();
            let b_relative = (reference + previous_offset).round();
            if a_relative < 0.0
                || b_relative < 0.0
                || a_relative >= source_span as f64
                || b_relative >= source_span as f64
            {
                continue;
            }
            let a_relative = a_relative as u64;
            let b_relative = b_relative as u64;
            let a_index = if reversed {
                source_offset as u64 + source_span as u64 - 1 - a_relative
            } else {
                source_offset as u64 + a_relative
            } as usize;
            let b_index = if reversed {
                source_offset as u64 + source_span as u64 - 1 - b_relative
            } else {
                source_offset as u64 + b_relative
            } as usize;
            let a_left = *left.add(a_index) as f64;
            let a_right = *right.add(a_index) as f64;
            let b_left = *left.add(b_index) as f64;
            let b_right = *right.add(b_index) as f64;
            if ![a_left, a_right, b_left, b_right]
                .iter()
                .all(|value| value.is_finite())
            {
                continue;
            }
            dot_left += a_left * b_left;
            energy_a_left += a_left * a_left;
            energy_b_left += b_left * b_left;
            dot_right += a_right * b_right;
            energy_a_right += a_right * a_right;
            energy_b_right += b_right * b_right;
        }

        let mut correlation = 0.0;
        let mut active_channels = 0;
        if energy_a_left > 1.0e-20 && energy_b_left > 1.0e-20 {
            correlation += (dot_left / (energy_a_left * energy_b_left).sqrt()).clamp(-1.0, 1.0);
            active_channels += 1;
        }
        if energy_a_right > 1.0e-20 && energy_b_right > 1.0e-20 {
            correlation += (dot_right / (energy_a_right * energy_b_right).sqrt()).clamp(-1.0, 1.0);
            active_channels += 1;
        }
        if active_channels == 0 {
            -2.0
        } else {
            correlation / active_channels as f64
        }
    };

    let mut best_correlation = -2.0;
    let mut selected = expected;
    let mut offset = -search_radius;
    while offset <= search_radius {
        if let Some(candidate) = center.checked_add(offset) {
            let correlation = score_candidate(candidate);
            if correlation > best_correlation {
                best_correlation = correlation;
                selected = candidate as f64;
            }
        }
        let Some(next) = offset.checked_add(4) else {
            break;
        };
        offset = next;
    }

    if best_correlation > -2.0 {
        let coarse_best = selected as i64;
        let fine_start = center
            .saturating_sub(search_radius)
            .max(coarse_best.saturating_sub(3));
        let fine_end = center
            .saturating_add(search_radius)
            .min(coarse_best.saturating_add(3));
        for candidate in fine_start..=fine_end {
            if candidate == coarse_best {
                continue;
            }
            let correlation = score_candidate(candidate);
            if correlation > best_correlation {
                best_correlation = correlation;
                selected = candidate as f64;
            }
        }
    }
    selected
}

/// Reads and overlap-adds the active WSOLA grains for one stereo output frame.
/// Grain scheduling remains with Track; interpolation, weighted accumulation
/// and normalization happen in this allocation-free Rust kernel.
#[no_mangle]
pub unsafe extern "C" fn hirari_wsola_render_frame(
    left: *const f32,
    right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    reverse: u8,
    resample_step: f64,
    kernel: *const f32,
    positions: *const f64,
    weights: *const f32,
    grain_count: u32,
    output: *mut f32,
) {
    if output.is_null() {
        return;
    }
    let output = slice::from_raw_parts_mut(output, 4);
    output.fill(0.0);
    if left.is_null()
        || right.is_null()
        || kernel.is_null()
        || positions.is_null()
        || weights.is_null()
        || !resample_step.is_finite()
        || grain_count as usize > MAX_ACTIVE_GRAINS
        || source_offset
            .checked_add(source_span)
            .map_or(true, |end| end > source_samples)
    {
        return;
    }

    let positions = slice::from_raw_parts(positions, grain_count as usize);
    let weights = slice::from_raw_parts(weights, grain_count as usize);
    let mut weight_sum = 0.0f32;
    for (&position, &weight) in positions.iter().zip(weights) {
        if !position.is_finite() || !weight.is_finite() {
            continue;
        }
        let mut left_sample = [0.0f32; 2];
        let mut right_sample = [0.0f32; 2];
        super::region_resampler::hirari_region_read_warped(
            left,
            source_samples,
            source_offset,
            source_span,
            position,
            reverse,
            0,
            resample_step,
            kernel,
            left_sample.as_mut_ptr(),
        );
        super::region_resampler::hirari_region_read_warped(
            right,
            source_samples,
            source_offset,
            source_span,
            position,
            reverse,
            0,
            resample_step,
            kernel,
            right_sample.as_mut_ptr(),
        );
        output[0] += left_sample[0] * weight;
        output[1] += left_sample[1] * weight;
        output[2] += right_sample[0] * weight;
        output[3] += right_sample[1] * weight;
        weight_sum += weight;
    }
    if weight_sum > 1.0e-6 {
        for sample in output.iter_mut() {
            *sample /= weight_sum;
        }
    }
}

/// Runs Track's region-level WSOLA scheduler for one output frame. Rust owns
/// grain pitch lookup, the bounded grain-start cache, correlation-search
/// inputs, overlap selection, and the final stereo frame render.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_wsola_frame(
    source_left: *const f32,
    source_right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    region_length: u64,
    loop_relative: u64,
    sample_rate: f64,
    source_rate: f64,
    base_pitch_ratio: f64,
    effective_pitch_ratio: f64,
    markers: *const HirariWarpMarker,
    marker_count: usize,
    note_ranges: *const HirariAudioNoteSegmentRange,
    note_curves: *const HirariAudioNoteCurveView,
    note_segment_count: usize,
    cache_grain_ids: *mut u64,
    cache_grain_starts: *mut f64,
    cache_capacity: usize,
    sync_group: u32,
    reverse: u8,
    kernel: *const f32,
    window: *const f32,
    output: *mut f32,
) {
    if output.is_null() {
        return;
    }
    let output = slice::from_raw_parts_mut(output, 4);
    output.fill(0.0);
    if source_left.is_null()
        || source_right.is_null()
        || kernel.is_null()
        || window.is_null()
        || cache_grain_ids.is_null()
        || cache_grain_starts.is_null()
        || source_span == 0
        || region_length == 0
        || source_offset
            .checked_add(source_span)
            .is_none_or(|end| end > source_samples)
        || note_segment_count > 0 && (note_ranges.is_null() || note_curves.is_null())
        || marker_count > 0 && markers.is_null()
        || ![
            sample_rate,
            source_rate,
            base_pitch_ratio,
            effective_pitch_ratio,
        ]
        .iter()
        .all(|value| value.is_finite())
        || base_pitch_ratio <= 0.0
        || effective_pitch_ratio <= 0.0
    {
        return;
    }

    const GRAIN_HOP: u64 = 256;
    const GRAIN_SIZE: u64 = 1024;
    const SEARCH_RADIUS: i64 = 64;
    const CORRELATION_POINTS: usize = (GRAIN_HOP / 8) as usize;
    let markers = if marker_count == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(markers, marker_count)
    };
    let note_ranges = if note_segment_count == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(note_ranges, note_segment_count)
    };
    let note_curves = if note_segment_count == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(note_curves, note_segment_count)
    };
    let cache_grain_ids = slice::from_raw_parts_mut(cache_grain_ids, cache_capacity);
    let cache_grain_starts = slice::from_raw_parts_mut(cache_grain_starts, cache_capacity);
    if cache_capacity == 0 || cache_capacity != cache_grain_starts.len() {
        return;
    }

    let source_position_at = |timeline_sample| unsafe {
        hirari_region_source_position_at(
            markers.as_ptr(),
            markers.len(),
            timeline_sample,
            source_rate,
            source_span,
        )
    };
    let source_rate_at = |timeline_sample: u64| {
        if timeline_sample.saturating_add(1) < region_length {
            source_position_at(timeline_sample + 1) - source_position_at(timeline_sample)
        } else if timeline_sample > 0 {
            source_position_at(timeline_sample) - source_position_at(timeline_sample - 1)
        } else {
            1.0
        }
    };
    let grain_pitch_ratio_at = |grain_id: u64| {
        let sample = grain_id.saturating_mul(GRAIN_HOP).min(region_length - 1);
        let seconds = sample as f64 / sample_rate.max(1.0);
        let mut indices = [-1i64; 3];
        hirari_audio_note_find_segment_ranges(
            note_ranges.as_ptr(),
            note_ranges.len(),
            seconds,
            indices.as_mut_ptr(),
        );
        let mut ratio = 1.0;
        if indices[2] >= 0 {
            let curve = &note_curves[indices[2] as usize];
            let mut pitch_formant = [0.0f64; 2];
            hirari_audio_note_curve_at(
                curve.anchors,
                curve.anchor_count,
                seconds,
                curve.pitch_offset_cents,
                0.0,
                pitch_formant.as_mut_ptr(),
            );
            if pitch_formant[0].is_finite() {
                ratio = 2.0f64.powf(pitch_formant[0].clamp(-4800.0, 4800.0) / 1200.0);
            }
        }
        (base_pitch_ratio * ratio).clamp(0.25, 4.0)
    };
    let mut grain_start = |grain_id: u64| {
        let cache_index = (grain_id % cache_capacity as u64) as usize;
        if cache_grain_ids[cache_index] == grain_id {
            return cache_grain_starts[cache_index];
        }
        let grain_pitch_ratio = grain_pitch_ratio_at(grain_id);
        let grain_output_start = grain_id.saturating_mul(GRAIN_HOP).min(region_length - 1);
        let expected = source_position_at(grain_output_start);
        let mut selected = expected;
        if sync_group == 0 && grain_id > 0 && expected < source_span as f64 {
            let previous_pitch_ratio = grain_pitch_ratio_at(grain_id - 1);
            let previous_output_start = (grain_id - 1).saturating_mul(GRAIN_HOP);
            let previous_expected = source_position_at(previous_output_start);
            let previous_output_end = previous_output_start
                .saturating_add(GRAIN_HOP)
                .min(region_length - 1);
            let previous_mapped_end = source_position_at(previous_output_end);
            let reference = previous_expected
                + (previous_mapped_end - previous_expected) * previous_pitch_ratio;
            let current_output_end = grain_output_start
                .saturating_add(GRAIN_HOP)
                .min(region_length - 1);
            let current_mapped_end = source_position_at(current_output_end);
            let overlap_span = (current_mapped_end - expected) * grain_pitch_ratio;
            let reference_span = (previous_mapped_end - previous_expected) * previous_pitch_ratio;
            let center = expected.round() as i64;
            let last = source_span.saturating_sub(1).min(i64::MAX as u64) as i64;
            let radius_adjustment = (GRAIN_HOP as f64
                * (source_rate_at(grain_output_start) - 1.0).abs())
            .round() as i64;
            let search_radius = SEARCH_RADIUS.saturating_add(radius_adjustment);
            let mut current_offsets = [0.0f64; CORRELATION_POINTS];
            let mut previous_offsets = [0.0f64; CORRELATION_POINTS];
            for point in 0..CORRELATION_POINTS {
                let offset = (point as u64) * 8;
                let current_timeline = grain_output_start
                    .saturating_add(offset)
                    .min(region_length - 1);
                let previous_timeline = previous_output_start
                    .saturating_add(offset)
                    .min(region_length - 1);
                current_offsets[point] =
                    (source_position_at(current_timeline) - expected) * grain_pitch_ratio;
                previous_offsets[point] = (source_position_at(previous_timeline)
                    - previous_expected)
                    * previous_pitch_ratio;
            }
            selected = hirari_wsola_select_grain(
                source_left,
                source_right,
                source_samples,
                source_offset,
                source_span,
                expected,
                reference,
                overlap_span,
                reference_span,
                center,
                last,
                search_radius,
                reverse,
                current_offsets.as_ptr(),
                previous_offsets.as_ptr(),
                CORRELATION_POINTS as u32,
            );
        }
        cache_grain_ids[cache_index] = grain_id;
        cache_grain_starts[cache_index] = selected;
        selected
    };

    let current_grain = loop_relative / GRAIN_HOP;
    let first_grain = current_grain.saturating_sub(3);
    let mut positions = [0.0f64; MAX_ACTIVE_GRAINS];
    let mut weights = [0.0f32; MAX_ACTIVE_GRAINS];
    let window = slice::from_raw_parts(window, GRAIN_SIZE as usize);
    let mut grain_count = 0usize;
    for grain in first_grain..=current_grain {
        let grain_output_start = grain.saturating_mul(GRAIN_HOP);
        let frame = loop_relative.saturating_sub(grain_output_start);
        if frame >= GRAIN_SIZE {
            continue;
        }
        let frame_timeline = grain_output_start
            .saturating_add(frame)
            .min(region_length - 1);
        let source_position = grain_start(grain)
            + (source_position_at(frame_timeline) - source_position_at(grain_output_start))
                * effective_pitch_ratio;
        if source_position < 0.0
            || source_position >= source_span as f64
            || grain_count >= MAX_ACTIVE_GRAINS
        {
            continue;
        }
        positions[grain_count] = source_position;
        weights[grain_count] = window[frame as usize];
        grain_count += 1;
    }
    hirari_wsola_render_frame(
        source_left,
        source_right,
        source_samples,
        source_offset,
        source_span,
        reverse,
        effective_pitch_ratio,
        kernel,
        positions.as_ptr(),
        weights.as_ptr(),
        grain_count as u32,
        output.as_mut_ptr(),
    );
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "wsola_differential_tests.rs"]
mod differential_tests;
