use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariWarpMarker {
    pub source_sample: u64,
    pub timeline_sample: u64,
    pub transient: u8,
}

/// Bounds and orders project-provided warp markers before they enter the
/// immutable audio-thread snapshot. Invalid or non-monotonic anchors are
/// dropped, matching Track's former C++ normalization contract.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_normalize_warp_markers(
    markers: *mut HirariWarpMarker,
    count: usize,
    source_span: u64,
    timeline_length: u64,
) -> usize {
    if count == 0 || markers.is_null() || timeline_length == 0 {
        return 0;
    }
    let markers = slice::from_raw_parts_mut(markers, count);
    markers.sort_by_key(|marker| marker.timeline_sample);
    let mut valid_count = 0usize;
    let mut previous_source = 0u64;
    let mut previous_timeline = 0u64;
    for index in 0..markers.len() {
        let marker = markers[index];
        if marker.source_sample > source_span
            || marker.timeline_sample > timeline_length
            || (valid_count > 0
                && (marker.source_sample <= previous_source
                    || marker.timeline_sample <= previous_timeline))
        {
            continue;
        }
        previous_source = marker.source_sample;
        previous_timeline = marker.timeline_sample;
        markers[valid_count] = marker;
        valid_count += 1;
    }
    valid_count
}

/// Maps one timeline frame to source-frame position using the region's
/// piecewise-linear warp markers. The marker storage stays owned by Track;
/// this function only reads the immutable published snapshot.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_source_position_at(
    markers: *const HirariWarpMarker,
    marker_count: usize,
    timeline_sample: u64,
    source_rate: f64,
    source_span: u64,
) -> f64 {
    if marker_count < 2 {
        return (timeline_sample as f64 * source_rate).clamp(0.0, source_span as f64);
    }
    if markers.is_null() {
        return 0.0;
    }
    let markers = slice::from_raw_parts(markers, marker_count);
    let upper = markers.partition_point(|marker| marker.timeline_sample <= timeline_sample);
    let (left, right) = if upper == 0 {
        (&markers[0], &markers[1])
    } else if upper >= markers.len() {
        (&markers[markers.len() - 2], &markers[markers.len() - 1])
    } else {
        (&markers[upper - 1], &markers[upper])
    };
    let timeline_delta = right.timeline_sample - left.timeline_sample;
    if timeline_delta == 0 {
        return left.source_sample as f64;
    }
    let source_per_timeline =
        (right.source_sample - left.source_sample) as f64 / timeline_delta as f64;
    let mapped = left.source_sample as f64
        + (timeline_sample as f64 - left.timeline_sample as f64) * source_per_timeline;
    mapped.clamp(0.0, source_span as f64)
}

/// Maps a contiguous, looped Track block to source positions and local source
/// rates. This avoids one C ABI crossing and marker search per rendered frame.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_source_block(
    markers: *const HirariWarpMarker,
    marker_count: usize,
    first_region_sample: u64,
    region_length: u64,
    source_rate: f64,
    source_span: u64,
    frame_count: u32,
    positions_output: *mut f64,
    rates_output: *mut f64,
) {
    if frame_count == 0 {
        return;
    }
    if positions_output.is_null() || rates_output.is_null() {
        return;
    }
    let frame_count = frame_count as usize;
    let positions = slice::from_raw_parts_mut(positions_output, frame_count);
    let rates = slice::from_raw_parts_mut(rates_output, frame_count);
    positions.fill(0.0);
    rates.fill(0.0);
    if region_length == 0 || (marker_count >= 2 && markers.is_null()) {
        return;
    }

    for frame in 0..frame_count {
        let Some(region_sample) = first_region_sample.checked_add(frame as u64) else {
            return;
        };
        let loop_sample = region_sample % region_length;
        let position = hirari_region_source_position_at(
            markers,
            marker_count,
            loop_sample,
            source_rate,
            source_span,
        );
        positions[frame] = position;
        rates[frame] = if loop_sample + 1 < region_length {
            hirari_region_source_position_at(
                markers,
                marker_count,
                loop_sample + 1,
                source_rate,
                source_span,
            ) - position
        } else if loop_sample > 0 {
            position
                - hirari_region_source_position_at(
                    markers,
                    marker_count,
                    loop_sample - 1,
                    source_rate,
                    source_span,
                )
        } else {
            1.0
        };
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::{hirari_region_normalize_warp_markers, HirariWarpMarker};

    #[test]
    fn warp_marker_normalization_bounds_and_enforces_monotonic_mapping() {
        let mut markers = [
            HirariWarpMarker {
                source_sample: 20,
                timeline_sample: 20,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 12,
                timeline_sample: 12,
                transient: 1,
            },
            HirariWarpMarker {
                source_sample: 7,
                timeline_sample: 8,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 30,
                timeline_sample: 30,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 4,
                timeline_sample: 12,
                transient: 0,
            },
        ];
        let count = unsafe {
            hirari_region_normalize_warp_markers(markers.as_mut_ptr(), markers.len(), 25, 25)
        };
        assert_eq!(count, 4);
        assert_eq!(
            markers[..count]
                .iter()
                .map(|marker| marker.timeline_sample)
                .collect::<Vec<_>>(),
            vec![0, 8, 12, 20]
        );
        assert_eq!(markers[2].transient, 1);
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "region_warp_differential_tests.rs"]
mod differential_tests;
