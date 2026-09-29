use crate::region_warp::HirariWarpMarker;

const ALIGNMENT_BINS: usize = 1024;
const MAX_ALIGNMENT_SAMPLES: usize = 16 * 1024 * 1024;
const MAX_ALIGNMENT_CHANNELS: usize = 32;
const MAX_OUTPUT_MARKERS: usize = 257;

unsafe fn normalized_envelope(
    channels: &[*const f32],
    source_offset: usize,
    source_span: usize,
) -> Option<[f64; ALIGNMENT_BINS]> {
    if channels.is_empty()
        || channels.len() > MAX_ALIGNMENT_CHANNELS
        || source_span < ALIGNMENT_BINS
        || source_span > MAX_ALIGNMENT_SAMPLES
    {
        return None;
    }
    let mut values = [0.0; ALIGNMENT_BINS];
    for (bin, value) in values.iter_mut().enumerate() {
        let begin = bin * source_span / ALIGNMENT_BINS;
        let end = (bin + 1) * source_span / ALIGNMENT_BINS;
        let step = ((end - begin) / 128).max(1);
        let mut energy = 0.0_f64;
        let mut count = 0_u64;
        for channel in channels.iter().copied() {
            if channel.is_null() {
                continue;
            }
            for frame in (begin..end).step_by(step) {
                let sample = unsafe { *channel.add(source_offset + frame) };
                if sample.is_finite() {
                    energy += sample as f64 * sample as f64;
                    count += 1;
                }
            }
        }
        let rms = if count > 0 {
            (energy / count as f64).sqrt()
        } else {
            0.0
        };
        *value = (rms * 32.0).ln_1p();
    }

    let mut smoothed = [0.0; ALIGNMENT_BINS];
    for (index, value) in smoothed.iter_mut().enumerate() {
        let left = index.saturating_sub(1);
        let right = (index + 1).min(ALIGNMENT_BINS - 1);
        *value = (values[left] + 2.0 * values[index] + values[right]) * 0.25;
    }
    let mean = smoothed.iter().sum::<f64>() / ALIGNMENT_BINS as f64;
    let variance = smoothed
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f64>()
        / ALIGNMENT_BINS as f64;
    let scale = variance.sqrt();
    if !scale.is_finite() || scale <= 1.0e-6 {
        return None;
    }
    for value in &mut smoothed {
        *value = (*value - mean) / scale;
    }
    Some(smoothed)
}

fn calculate_warp(
    reference: &[f64; ALIGNMENT_BINS],
    target: &[f64; ALIGNMENT_BINS],
    target_span: u64,
    target_timeline_length: u64,
) -> Option<Vec<HirariWarpMarker>> {
    let side = ALIGNMENT_BINS + 1;
    let mut cost = vec![f64::INFINITY; side * side];
    let at = |row: usize, col: usize| row * side + col;
    cost[0] = 0.0;
    for row in 1..=ALIGNMENT_BINS {
        for col in 1..=ALIGNMENT_BINS {
            let local = (reference[row - 1] - target[col - 1]).abs();
            cost[at(row, col)] = local
                + cost[at(row - 1, col - 1)]
                    .min((cost[at(row - 1, col)] + 0.08).min(cost[at(row, col - 1)] + 0.08));
        }
    }

    let mut row_sums = [0_u64; ALIGNMENT_BINS + 1];
    let mut row_counts = [0_u32; ALIGNMENT_BINS + 1];
    let (mut row, mut col) = (ALIGNMENT_BINS, ALIGNMENT_BINS);
    while row > 0 || col > 0 {
        if col <= ALIGNMENT_BINS {
            row_sums[col] += row as u64;
            row_counts[col] += 1;
        }
        if row == 0 {
            col -= 1;
            continue;
        }
        if col == 0 {
            row -= 1;
            continue;
        }
        let diagonal = cost[at(row - 1, col - 1)];
        let vertical = cost[at(row - 1, col)] + 0.08;
        let horizontal = cost[at(row, col - 1)] + 0.08;
        if diagonal <= vertical && diagonal <= horizontal {
            row -= 1;
            col -= 1;
        } else if vertical <= horizontal {
            row -= 1;
        } else {
            col -= 1;
        }
    }
    row_sums[0] = 0;
    row_counts[0] = 1;
    row_sums[ALIGNMENT_BINS] = ALIGNMENT_BINS as u64;
    row_counts[ALIGNMENT_BINS] = 1;

    let mut mapped_rows = [0.0; ALIGNMENT_BINS + 1];
    let mut present = [false; ALIGNMENT_BINS + 1];
    for index in 0..=ALIGNMENT_BINS {
        if row_counts[index] != 0 {
            mapped_rows[index] = row_sums[index] as f64 / row_counts[index] as f64;
            present[index] = true;
        }
    }
    for index in 1..ALIGNMENT_BINS {
        if present[index] {
            continue;
        }
        let mut left = index - 1;
        while left > 0 && !present[left] {
            left -= 1;
        }
        let mut right = index + 1;
        while right < ALIGNMENT_BINS && !present[right] {
            right += 1;
        }
        let fraction = (index - left) as f64 / (right - left) as f64;
        mapped_rows[index] =
            mapped_rows[left] + (mapped_rows[right] - mapped_rows[left]) * fraction;
    }

    let mut markers = Vec::with_capacity(MAX_OUTPUT_MARKERS);
    markers.push(HirariWarpMarker::default());
    let (mut previous_source, mut previous_timeline) = (0_u64, 0_u64);
    for bin in (4..ALIGNMENT_BINS).step_by(4) {
        let source = bin as u64 * target_span / ALIGNMENT_BINS as u64;
        let timeline = (mapped_rows[bin] * target_timeline_length as f64 / ALIGNMENT_BINS as f64)
            .round() as u64;
        if source <= previous_source
            || timeline <= previous_timeline
            || timeline >= target_timeline_length
        {
            continue;
        }
        let local_ratio = (source - previous_source) as f64 / (timeline - previous_timeline) as f64;
        if !local_ratio.is_finite() || !(0.25..=4.0).contains(&local_ratio) {
            continue;
        }
        markers.push(HirariWarpMarker {
            source_sample: source,
            timeline_sample: timeline,
            transient: 1,
        });
        previous_source = source;
        previous_timeline = timeline;
    }
    if target_span <= previous_source || target_timeline_length <= previous_timeline {
        return None;
    }
    let final_ratio = (target_span - previous_source) as f64
        / (target_timeline_length - previous_timeline) as f64;
    if !final_ratio.is_finite() || !(0.25..=4.0).contains(&final_ratio) {
        return None;
    }
    markers.push(HirariWarpMarker {
        source_sample: target_span,
        timeline_sample: target_timeline_length,
        transient: 0,
    });
    (markers.len() >= 2).then_some(markers)
}

/// Builds normalized energy envelopes and computes constrained DTW warp markers.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_align_audio(
    reference_channels: *const *const f32,
    reference_channel_count: usize,
    reference_offset: usize,
    reference_span: usize,
    target_channels: *const *const f32,
    target_channel_count: usize,
    target_offset: usize,
    target_span: usize,
    target_timeline_length: u64,
    output_markers: *mut HirariWarpMarker,
    output_capacity: usize,
    output_count: *mut usize,
) -> bool {
    if reference_channels.is_null()
        || target_channels.is_null()
        || output_markers.is_null()
        || output_count.is_null()
        || output_capacity < MAX_OUTPUT_MARKERS
        || target_timeline_length == 0
    {
        return false;
    }
    let reference_channels =
        std::slice::from_raw_parts(reference_channels, reference_channel_count);
    let target_channels = std::slice::from_raw_parts(target_channels, target_channel_count);
    let Some(reference) =
        (unsafe { normalized_envelope(reference_channels, reference_offset, reference_span) })
    else {
        return false;
    };
    let Some(target) =
        (unsafe { normalized_envelope(target_channels, target_offset, target_span) })
    else {
        return false;
    };
    let Some(markers) = calculate_warp(
        &reference,
        &target,
        target_span as u64,
        target_timeline_length,
    ) else {
        return false;
    };
    std::slice::from_raw_parts_mut(output_markers, markers.len()).copy_from_slice(&markers);
    *output_count = markers.len();
    true
}

#[cfg(test)]
mod tests {
    use super::hirari_region_align_audio;
    use crate::region_warp::HirariWarpMarker;

    #[test]
    fn identical_audio_produces_identity_alignment_endpoints() {
        let samples: Vec<f32> = (0..4096)
            .map(|index| {
                let bin = index / 4;
                if (bin / 32) % 2 == 0 {
                    0.1
                } else {
                    0.8
                }
            })
            .collect();
        let channels = [samples.as_ptr()];
        let mut markers = [HirariWarpMarker::default(); 257];
        let mut count = 0;
        assert!(unsafe {
            hirari_region_align_audio(
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                samples.len() as u64,
                markers.as_mut_ptr(),
                markers.len(),
                &mut count,
            )
        });
        assert_eq!(markers[0].source_sample, 0);
        assert_eq!(markers[0].timeline_sample, 0);
        assert_eq!(markers[count - 1].source_sample, samples.len() as u64);
        assert_eq!(markers[count - 1].timeline_sample, samples.len() as u64);
        assert!(markers[..count]
            .iter()
            .all(|marker| marker.source_sample == marker.timeline_sample));
        assert!(markers[..count].windows(2).all(|pair| {
            pair[0].source_sample < pair[1].source_sample
                && pair[0].timeline_sample < pair[1].timeline_sample
        }));
    }

    #[test]
    fn constant_audio_and_invalid_output_capacity_are_rejected() {
        let samples = vec![0.25_f32; 4096];
        let channels = [samples.as_ptr()];
        let mut markers = [HirariWarpMarker::default(); 257];
        let mut count = 0;
        assert!(!unsafe {
            hirari_region_align_audio(
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                samples.len() as u64,
                markers.as_mut_ptr(),
                markers.len(),
                &mut count,
            )
        });
        assert!(!unsafe {
            hirari_region_align_audio(
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                channels.as_ptr(),
                1,
                0,
                samples.len(),
                samples.len() as u64,
                markers.as_mut_ptr(),
                markers.len() - 1,
                &mut count,
            )
        });
    }
}
