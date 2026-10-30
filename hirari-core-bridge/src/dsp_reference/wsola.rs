//! Frozen WSOLA correlation search and grain renderer.

use super::region_resampler::hirari_region_read_warped_reference;

#[no_mangle]
pub unsafe extern "C" fn hirari_wsola_select_grain_reference(
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
    mut last: i64,
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
        || source_offset > source_samples
        || source_span > source_samples - source_offset
        || !expected.is_finite()
        || !reference.is_finite()
        || !overlap_span.is_finite()
        || !reference_span.is_finite()
        || search_radius < 0
        || point_count == 0
        || point_count > 32
    {
        return expected;
    }

    last = last.min((source_span - 1) as i64);
    let score_candidate = |candidate: i64| -> f64 {
        if candidate < 0
            || candidate as f64 + overlap_span >= last as f64
            || reference < 0.0
            || reference + reference_span >= last as f64
        {
            return -2.0;
        }
        let mut dot_l = 0.0f64;
        let mut energy_al = 0.0f64;
        let mut energy_bl = 0.0f64;
        let mut dot_r = 0.0f64;
        let mut energy_ar = 0.0f64;
        let mut energy_br = 0.0f64;
        for i in 0..point_count as usize {
            let a_rel = (candidate as f64 + *current_offsets.add(i)).round();
            let b_rel = (reference + *previous_offsets.add(i)).round();
            if a_rel < 0.0
                || b_rel < 0.0
                || a_rel >= source_span as f64
                || b_rel >= source_span as f64
            {
                continue;
            }
            let a_relative = a_rel as u64;
            let b_relative = b_rel as u64;
            let a_index = if reverse != 0 {
                source_offset + source_span - 1 - a_relative
            } else {
                source_offset + a_relative
            };
            let b_index = if reverse != 0 {
                source_offset + source_span - 1 - b_relative
            } else {
                source_offset + b_relative
            };
            if a_index >= source_samples || b_index >= source_samples {
                continue;
            }
            let a_l = *left.add(a_index as usize) as f64;
            let a_r = *right.add(a_index as usize) as f64;
            let b_l = *left.add(b_index as usize) as f64;
            let b_r = *right.add(b_index as usize) as f64;
            if !a_l.is_finite()
                || !a_r.is_finite()
                || !b_l.is_finite()
                || !b_r.is_finite()
            {
                continue;
            }
            dot_l += a_l * b_l;
            energy_al += a_l * a_l;
            energy_bl += b_l * b_l;
            dot_r += a_r * b_r;
            energy_ar += a_r * a_r;
            energy_br += b_r * b_r;
        }
        let mut correlation = 0.0f64;
        let mut active_channels = 0u32;
        if energy_al > 1.0e-20 && energy_bl > 1.0e-20 {
            correlation += (dot_l / (energy_al * energy_bl).sqrt()).clamp(-1.0, 1.0);
            active_channels += 1;
        }
        if energy_ar > 1.0e-20 && energy_br > 1.0e-20 {
            correlation += (dot_r / (energy_ar * energy_br).sqrt()).clamp(-1.0, 1.0);
            active_channels += 1;
        }
        if active_channels == 0 {
            -2.0
        } else {
            correlation / active_channels as f64
        }
    };

    let mut best_correlation = -2.0f64;
    let mut selected = expected;
    let mut offset = -search_radius;
    while offset <= search_radius {
        let candidate = center + offset;
        let correlation = score_candidate(candidate);
        if correlation > best_correlation {
            best_correlation = correlation;
            selected = candidate as f64;
        }
        offset += 4;
    }
    if best_correlation > -2.0 {
        let coarse_best = selected as i64;
        let fine_start = (center - search_radius).max(coarse_best - 3);
        let fine_end = (center + search_radius).min(coarse_best + 3);
        let mut candidate = fine_start;
        while candidate <= fine_end {
            if candidate != coarse_best {
                let correlation = score_candidate(candidate);
                if correlation > best_correlation {
                    best_correlation = correlation;
                    selected = candidate as f64;
                }
            }
            candidate += 1;
        }
    }
    selected
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wsola_render_frame_reference(
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
    for index in 0..4 {
        *output.add(index) = 0.0;
    }
    if left.is_null()
        || right.is_null()
        || kernel.is_null()
        || positions.is_null()
        || weights.is_null()
        || grain_count > 4
    {
        return;
    }
    let mut weight_sum = 0.0f32;
    for grain in 0..grain_count as usize {
        let mut left_sample = [0.0f32; 2];
        let mut right_sample = [0.0f32; 2];
        hirari_region_read_warped_reference(
            left,
            source_samples,
            source_offset,
            source_span,
            *positions.add(grain),
            reverse,
            0,
            resample_step,
            kernel,
            left_sample.as_mut_ptr(),
        );
        hirari_region_read_warped_reference(
            right,
            source_samples,
            source_offset,
            source_span,
            *positions.add(grain),
            reverse,
            0,
            resample_step,
            kernel,
            right_sample.as_mut_ptr(),
        );
        let weight = *weights.add(grain);
        *output.add(0) += left_sample[0] * weight;
        *output.add(1) += left_sample[1] * weight;
        *output.add(2) += right_sample[0] * weight;
        *output.add(3) += right_sample[1] * weight;
        weight_sum += weight;
    }
    if weight_sum > 1.0e-6 {
        for index in 0..4 {
            *output.add(index) /= weight_sum;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_wsola_frame_reference(
    left: *const f32,
    right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    region_length: u64,
    loop_relative: u64,
    _sample_rate: f64,
    source_rate: f64,
    base_pitch_ratio: f64,
    effective_pitch_ratio: f64,
    cache_ids: *mut u64,
    cache_starts: *mut f64,
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
    for index in 0..4 {
        *output.add(index) = 0.0;
    }
    if left.is_null()
        || right.is_null()
        || cache_ids.is_null()
        || cache_starts.is_null()
        || kernel.is_null()
        || window.is_null()
        || cache_capacity == 0
        || source_span == 0
        || region_length == 0
    {
        return;
    }

    const HOP: u64 = 256;
    const GRAIN_SIZE: u64 = 1024;
    const RADIUS: i64 = 64;

    let source_position = |sample: u64| -> f64 {
        (sample as f64 * source_rate).clamp(0.0, source_span as f64)
    };
    let pitch_ratio = |_grain: u64| -> f64 { base_pitch_ratio };

    let cache_ids = std::slice::from_raw_parts_mut(cache_ids, cache_capacity);
    let cache_starts = std::slice::from_raw_parts_mut(cache_starts, cache_capacity);

    let grain_start = |grain: u64, cache_ids: &mut [u64], cache_starts: &mut [f64]| -> f64 {
        let index = (grain % cache_capacity as u64) as usize;
        if cache_ids[index] == grain {
            return cache_starts[index];
        }
        let output_start = (grain * HOP).min(region_length - 1);
        let expected = source_position(output_start);
        let mut selected = expected;
        let grain_ratio = pitch_ratio(grain);
        if sync_group == 0 && grain > 0 && expected < source_span as f64 {
            let previous_ratio = pitch_ratio(grain - 1);
            let previous_start = (grain - 1) * HOP;
            let previous_expected = source_position(previous_start);
            let previous_end = (previous_start + HOP).min(region_length - 1);
            let previous_mapped_end = source_position(previous_end);
            let reference =
                previous_expected + (previous_mapped_end - previous_expected) * previous_ratio;
            let current_end = (output_start + HOP).min(region_length - 1);
            let current_mapped_end = source_position(current_end);
            let overlap = (current_mapped_end - expected) * grain_ratio;
            let reference_span = (previous_mapped_end - previous_expected) * previous_ratio;
            let center = expected.round() as i64;
            let last = (source_span - 1) as i64;
            let search_radius =
                RADIUS + (HOP as f64 * (source_rate - 1.0).abs()).round() as i64;
            let mut current_offsets = [0.0f64; 32];
            let mut previous_offsets = [0.0f64; 32];
            let mut point = 0u64;
            while point < HOP {
                let i = (point / 8) as usize;
                let current_sample = (output_start + point).min(region_length - 1);
                let previous_sample = (previous_start + point).min(region_length - 1);
                current_offsets[i] = (source_position(current_sample) - expected) * grain_ratio;
                previous_offsets[i] =
                    (source_position(previous_sample) - previous_expected) * previous_ratio;
                point += 8;
            }
            selected = hirari_wsola_select_grain_reference(
                left,
                right,
                source_samples,
                source_offset,
                source_span,
                expected,
                reference,
                overlap,
                reference_span,
                center,
                last,
                search_radius,
                reverse,
                current_offsets.as_ptr(),
                previous_offsets.as_ptr(),
                32,
            );
        }
        cache_ids[index] = grain;
        cache_starts[index] = selected;
        selected
    };

    let current = loop_relative / HOP;
    let first = if current > 3 { current - 3 } else { 0 };
    let mut positions = [0.0f64; 4];
    let mut weights = [0.0f32; 4];
    let mut count = 0u32;
    let mut grain = first;
    while grain <= current {
        let start = grain * HOP;
        let frame = loop_relative - start;
        if frame < GRAIN_SIZE {
            let frame_sample = (start + frame).min(region_length - 1);
            let position = grain_start(grain, cache_ids, cache_starts)
                + (source_position(frame_sample) - source_position(start)) * effective_pitch_ratio;
            if !(position < 0.0 || position >= source_span as f64 || count >= 4) {
                positions[count as usize] = position;
                weights[count as usize] = *window.add(frame as usize);
                count += 1;
            }
        }
        grain += 1;
    }
    hirari_wsola_render_frame_reference(
        left,
        right,
        source_samples,
        source_offset,
        source_span,
        reverse,
        effective_pitch_ratio,
        kernel,
        positions.as_ptr(),
        weights.as_ptr(),
        count,
        output,
    );
}
