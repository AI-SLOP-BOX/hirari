//! Frozen warp-map and spectral-stretch policy used by the region render tests.

use crate::audio_note_curve::HirariAudioNoteCurveView;
use crate::region_warp::HirariWarpMarker;

#[no_mangle]
pub unsafe extern "C" fn hirari_region_needs_spectral_stretch_reference(
    source_rate: f64,
    warp_marker_count: usize,
    pitch_semitones: f32,
    note_curves: *const HirariAudioNoteCurveView,
    note_curve_count: usize,
) -> bool {
    let needed = (source_rate - 1.0).abs() > 1.0e-5
        || warp_marker_count != 0
        || pitch_semitones.abs() > 1.0e-4;

    if !note_curves.is_null() && note_curve_count != 0 {
        let curves = std::slice::from_raw_parts(note_curves, note_curve_count);
        for curve in curves {
            if curve.pitch_offset_cents.abs() > 1.0e-3
                || curve.formant_offset_cents.abs() > 1.0e-3
            {
                return true;
            }
            if !curve.anchors.is_null() && curve.anchor_count != 0 {
                let anchors = std::slice::from_raw_parts(curve.anchors, curve.anchor_count);
                for anchor in anchors {
                    if anchor.pitch_cents.abs() > 1.0e-3 || anchor.formant_cents.abs() > 1.0e-3 {
                        return true;
                    }
                }
            }
        }
    }
    needed
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_source_position_reference(
    markers: *const HirariWarpMarker,
    marker_count: usize,
    timeline_sample: u64,
    source_rate: f64,
    source_span: u64,
) -> f64 {
    if marker_count < 2 || markers.is_null() {
        return (timeline_sample as f64 * source_rate).clamp(0.0, source_span as f64);
    }
    let markers = std::slice::from_raw_parts(markers, marker_count);
    let upper = markers.partition_point(|marker| marker.timeline_sample <= timeline_sample);
    let (left, right) = if upper == 0 {
        (&markers[0], &markers[1])
    } else if upper == marker_count {
        (&markers[marker_count - 2], &markers[marker_count - 1])
    } else {
        (&markers[upper - 1], &markers[upper])
    };
    let timeline_delta = right.timeline_sample.wrapping_sub(left.timeline_sample);
    if timeline_delta == 0 {
        return left.source_sample as f64;
    }
    let source_per_timeline = right.source_sample.wrapping_sub(left.source_sample) as f64
        / timeline_delta as f64;
    let mapped = left.source_sample as f64
        + (timeline_sample as f64 - left.timeline_sample as f64) * source_per_timeline;
    mapped.clamp(0.0, source_span as f64)
}
