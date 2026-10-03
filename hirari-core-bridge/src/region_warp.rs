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
/// Pure transform: reads the caller's markers and returns a buffer it owns.
///
/// Rust never writes into caller storage here. The previous signature took a
/// mutable pointer, sorted and compacted in place, and returned a count that
/// the caller then turned into a container resize, which is the same
/// architecture that silently zeroed an appended region edit once. Marker
/// storage still belongs to Track; only the normalization result is Rust-owned.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_normalize_warp_markers(
    markers_in: *const HirariWarpMarker,
    count: usize,
    source_span: u64,
    timeline_length: u64,
    out_count: *mut usize,
) -> *mut HirariWarpMarker {
    if out_count.is_null() {
        return std::ptr::null_mut();
    }
    if count == 0 || markers_in.is_null() || timeline_length == 0 {
        unsafe { *out_count = 0 };
        return std::ptr::null_mut();
    }
    let mut markers = unsafe { slice::from_raw_parts(markers_in, count) }.to_vec();
    markers.sort_by_key(|marker| marker.timeline_sample);
    let mut valid: Vec<HirariWarpMarker> = Vec::with_capacity(markers.len());
    let mut previous_source = 0u64;
    let mut previous_timeline = 0u64;
    for marker in markers {
        if marker.source_sample > source_span
            || marker.timeline_sample > timeline_length
            || (!valid.is_empty()
                && (marker.source_sample <= previous_source
                    || marker.timeline_sample <= previous_timeline))
        {
            continue;
        }
        previous_source = marker.source_sample;
        previous_timeline = marker.timeline_sample;
        valid.push(marker);
    }
    let length = valid.len();
    unsafe { *out_count = length };
    Box::into_raw(valid.into_boxed_slice()) as *mut HirariWarpMarker
}

/// Releases a buffer returned by [`hirari_region_normalize_warp_markers`].
#[no_mangle]
pub unsafe extern "C" fn hirari_free_warp_markers(markers: *mut HirariWarpMarker, count: usize) {
    if markers.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(slice::from_raw_parts_mut(markers, count)));
    }
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
    use std::slice;

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
        let mut count = 0usize;
        let normalized = unsafe {
            hirari_region_normalize_warp_markers(
                markers.as_ptr(),
                markers.len(),
                25,
                25,
                &mut count,
            )
        };
        assert_eq!(count, 4);
        let result = unsafe { slice::from_raw_parts(normalized, count) }.to_vec();
        assert_eq!(
            result
                .iter()
                .map(|marker| marker.timeline_sample)
                .collect::<Vec<_>>(),
            vec![0, 8, 12, 20]
        );
        assert_eq!(result[2].transient, 1);
        unsafe { super::hirari_free_warp_markers(normalized, count) };
        assert_eq!(markers.len(), 6, "input must not be modified in place");


    }
    /// Pins the contract the C++ caller depends on: the transform keeps every
    /// marker it returns in strictly increasing order, and drops anything it
    /// cannot keep. The caller compares the returned count against its input
    /// count to decide whether the edit is acceptable, so dropping exactly one
    /// marker must produce a different count rather than a silently truncated
    /// result of the same length.
    #[test]
    fn normalization_drops_only_invalid_markers_and_reports_the_count() {
        let valid = [
            HirariWarpMarker { source_sample: 0, timeline_sample: 0, transient: 0 },
            HirariWarpMarker { source_sample: 512, timeline_sample: 512, transient: 1 },
            HirariWarpMarker { source_sample: 1024, timeline_sample: 1024, transient: 0 },
        ];
        let mut count = 0usize;
        let out = unsafe {
            hirari_region_normalize_warp_markers(valid.as_ptr(), valid.len(), 4096, 4096, &mut count)
        };
        assert_eq!(count, 3);
        unsafe { super::hirari_free_warp_markers(out, count) };

        // Unordered but valid input is sorted and kept, not dropped. The C++
        // caller accepts exactly this case by comparing counts, so the output
        // order, and not just the count, is the contract that matters here.
        let unordered = [
            HirariWarpMarker { source_sample: 0, timeline_sample: 0, transient: 0 },
            HirariWarpMarker { source_sample: 1024, timeline_sample: 1024, transient: 0 },
            HirariWarpMarker { source_sample: 512, timeline_sample: 512, transient: 0 },
        ];
        let mut reordered = 0usize;
        let out = unsafe {
            hirari_region_normalize_warp_markers(
                unordered.as_ptr(), unordered.len(), 4096, 4096, &mut reordered,
            )
        };
        assert_eq!(reordered, 3);
        let ordered = unsafe { std::slice::from_raw_parts(out, reordered) };
        assert_eq!(
            ordered.iter().map(|m| m.timeline_sample).collect::<Vec<_>>(),
            vec![0, 512, 1024],
            "the output is sorted even though nothing was dropped"
        );
        unsafe { super::hirari_free_warp_markers(out, reordered) };

        // A marker that duplicates its predecessor is not strictly increasing
        // and is the case that actually changes the count.
        let duplicate = [
            HirariWarpMarker { source_sample: 0, timeline_sample: 0, transient: 0 },
            HirariWarpMarker { source_sample: 1024, timeline_sample: 1024, transient: 0 },
            HirariWarpMarker { source_sample: 1024, timeline_sample: 1024, transient: 0 },
        ];
        let mut dropped = 0usize;
        let out = unsafe {
            hirari_region_normalize_warp_markers(
                duplicate.as_ptr(), duplicate.len(), 4096, 4096, &mut dropped,
            )
        };
        assert_eq!(dropped, 2, "one marker must be dropped, changing the count");
        unsafe { super::hirari_free_warp_markers(out, dropped) };

        // Beyond the timeline bound.
        let out_of_range = [HirariWarpMarker { source_sample: 0, timeline_sample: 999, transient: 0 }];
        let mut rejected = 0usize;
        let out = unsafe {
            hirari_region_normalize_warp_markers(
                out_of_range.as_ptr(), out_of_range.len(), 4096, 100, &mut rejected,
            )
        };
        assert_eq!(rejected, 0, "the out-of-range marker must be dropped");
        unsafe { super::hirari_free_warp_markers(out, rejected) };
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "region_warp_differential_tests.rs"]
mod differential_tests;
