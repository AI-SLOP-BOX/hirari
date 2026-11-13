use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNoteAnchor {
    pub position_seconds: f64,
    pub pitch_cents: f64,
    pub formant_cents: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNoteSegmentRange {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub detected_pitch_cents: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNoteSegmentValidation {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub detected_pitch_cents: f64,
    pub pitch_offset_cents: f64,
    pub formant_offset_cents: f64,
    pub anchors: *const HirariAudioNoteAnchor,
    pub anchor_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNotePhaseView {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub pitch_offset_cents: f64,
    pub anchors: *const HirariAudioNoteAnchor,
    pub anchor_count: usize,
    pub integral_prefix: *const f64,
}

/// Drops invalid/out-of-segment anchors and orders the surviving curve points.
/// The caller provides capacity for every input anchor and applies the count.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_normalize_anchors(
    anchors: *mut HirariAudioNoteAnchor,
    anchor_count: usize,
    segment_start: f64,
    segment_end: f64,
) -> usize {
    if anchor_count > 4096
        || (anchor_count > 0 && anchors.is_null())
        || !segment_start.is_finite()
        || !segment_end.is_finite()
        || segment_end <= segment_start
    {
        return usize::MAX;
    }
    if anchor_count == 0 {
        return 0;
    }
    let anchors = slice::from_raw_parts_mut(anchors, anchor_count);
    let mut valid_count = 0;
    for index in 0..anchor_count {
        let anchor = anchors[index];
        if anchor.position_seconds.is_finite()
            && anchor.pitch_cents.is_finite()
            && anchor.formant_cents.is_finite()
            && (segment_start..=segment_end).contains(&anchor.position_seconds)
        {
            anchors[valid_count] = anchor;
            valid_count += 1;
        }
    }
    anchors[..valid_count].sort_by(|left, right| {
        left.position_seconds
            .partial_cmp(&right.position_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    valid_count
}

/// Validates a batch of note edits and returns their stable chronological order.
/// Segment ownership and integral-prefix rebuilding stay with the caller; this
/// function owns the shared edit-data invariants used before a batch is stored.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_validate_segments(
    segments: *const HirariAudioNoteSegmentValidation,
    segment_count: usize,
    ordered_indices: *mut u32,
) -> bool {
    if segment_count > 4096
        || (segment_count > 0 && (segments.is_null() || ordered_indices.is_null()))
    {
        return false;
    }
    if segment_count == 0 {
        return true;
    }
    let segments = slice::from_raw_parts(segments, segment_count);
    let output = slice::from_raw_parts_mut(ordered_indices, segment_count);
    for (index, segment) in segments.iter().enumerate() {
        if !segment.start_seconds.is_finite()
            || !segment.end_seconds.is_finite()
            || segment.end_seconds <= segment.start_seconds
            || !segment.detected_pitch_cents.is_finite()
            || !segment.pitch_offset_cents.is_finite()
            || !segment.formant_offset_cents.is_finite()
            || segment.anchor_count > 4096
            || (segment.anchor_count > 0 && segment.anchors.is_null())
        {
            return false;
        }
        if segment.anchor_count == 0 {
            output[index] = index as u32;
            continue;
        }
        let anchors = slice::from_raw_parts(segment.anchors, segment.anchor_count);
        let mut previous = f64::NEG_INFINITY;
        for anchor in anchors {
            if !anchor.position_seconds.is_finite()
                || !anchor.pitch_cents.is_finite()
                || !anchor.formant_cents.is_finite()
                || anchor.position_seconds < segment.start_seconds
                || anchor.position_seconds > segment.end_seconds
                || anchor.position_seconds < previous
            {
                return false;
            }
            previous = anchor.position_seconds;
        }
        output[index] = index as u32;
    }
    output.sort_by(|left, right| {
        segments[*left as usize]
            .start_seconds
            .total_cmp(&segments[*right as usize].start_seconds)
            .then_with(|| left.cmp(right))
    });
    for pair in output.windows(2) {
        if segments[pair[0] as usize].end_seconds > segments[pair[1] as usize].start_seconds {
            return false;
        }
    }
    true
}

/// Returns the replacement index for a segment upsert, `-1` for insertion,
/// and `-2` when the requested interval conflicts with an existing segment.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_upsert_segment_slot(
    ranges: *const HirariAudioNoteSegmentRange,
    range_count: usize,
    start_seconds: f64,
    end_seconds: f64,
) -> i64 {
    if !start_seconds.is_finite()
        || !end_seconds.is_finite()
        || end_seconds <= start_seconds
        || (range_count > 0 && ranges.is_null())
    {
        return -2;
    }
    let mut replacement = -1;
    if range_count == 0 {
        return replacement;
    }
    for (index, current) in slice::from_raw_parts(ranges, range_count)
        .iter()
        .enumerate()
    {
        let same_start = (current.start_seconds - start_seconds).abs() < 1.0e-9;
        let overlaps = start_seconds < current.end_seconds && current.start_seconds < end_seconds;
        if overlaps && !same_start {
            return -2;
        }
        if same_start && replacement < 0 {
            replacement = index as i64;
        }
    }
    replacement
}

/// Locates the first segment whose start matches the editor's 1 ns tolerance.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_find_segment_for_edit(
    ranges: *const HirariAudioNoteSegmentRange,
    range_count: usize,
    start_seconds: f64,
) -> i64 {
    if !start_seconds.is_finite() || (range_count > 0 && ranges.is_null()) {
        return -1;
    }
    if range_count == 0 {
        return -1;
    }
    slice::from_raw_parts(ranges, range_count)
        .iter()
        .position(|range| (range.start_seconds - start_seconds).abs() < 1.0e-9)
        .map_or(-1, |index| index as i64)
}

/// Checks whether a timing edit would overlap any segment except its target.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_can_warp_segment(
    ranges: *const HirariAudioNoteSegmentRange,
    range_count: usize,
    target_index: usize,
    new_start_seconds: f64,
    new_end_seconds: f64,
) -> bool {
    if !new_start_seconds.is_finite()
        || !new_end_seconds.is_finite()
        || new_end_seconds <= new_start_seconds
        || target_index >= range_count
        || ranges.is_null()
    {
        return false;
    }
    slice::from_raw_parts(ranges, range_count)
        .iter()
        .enumerate()
        .all(|(index, current)| {
            index == target_index
                || !(new_start_seconds < current.end_seconds
                    && current.start_seconds < new_end_seconds)
        })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNoteCurveView {
    pub anchors: *const HirariAudioNoteAnchor,
    pub anchor_count: usize,
    pub integral_prefix: *const f64,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub pitch_offset_cents: f64,
    pub correction_before_seconds: f64,
    pub formant_offset_cents: f64,
}

/// Finds the active edit using Track's historical overlap rule, the immediately
/// preceding ended note used for phase continuity, and the newest containing
/// segment used by WSOLA grain pitch lookup.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_find_segment_ranges(
    ranges: *const HirariAudioNoteSegmentRange,
    range_count: usize,
    seconds: f64,
    output_indices: *mut i64,
) {
    if output_indices.is_null() {
        return;
    }
    let output = slice::from_raw_parts_mut(output_indices, 3);
    output.fill(-1);
    if ranges.is_null() || range_count == 0 || !seconds.is_finite() {
        return;
    }
    let ranges = slice::from_raw_parts(ranges, range_count);
    output.copy_from_slice(&find_segment_indices(ranges, seconds));
}

#[inline]
fn find_segment_indices(ranges: &[HirariAudioNoteSegmentRange], seconds: f64) -> [i64; 3] {
    let mut output = [-1; 3];
    if ranges.is_empty() || !seconds.is_finite() {
        return output;
    }
    let upper = ranges.partition_point(|range| range.start_seconds <= seconds);
    if upper == 0 {
        return output;
    }
    let latest = upper - 1;
    if ranges[latest].end_seconds < seconds {
        output[1] = latest as i64;
    }
    if seconds <= ranges[latest].end_seconds {
        output[2] = latest as i64;
    }

    let mut cursor = upper;
    while cursor > 0 {
        let index = cursor - 1;
        let range = &ranges[index];
        if range.end_seconds < seconds {
            break;
        }
        if seconds >= range.start_seconds && seconds <= range.end_seconds {
            output[0] = index as i64;
        }
        cursor = index;
    }
    output
}

/// Evaluates the active pitch and formant edit for a contiguous Track block.
/// This moves the realtime per-frame lookup and curve interpolation into Rust
/// while returning the matched/previous range indices needed by the optional
/// phase-continuous pitch-correction renderer.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_block(
    ranges: *const HirariAudioNoteSegmentRange,
    range_count: usize,
    curves: *const HirariAudioNoteCurveView,
    curve_count: usize,
    first_region_sample: u64,
    region_length: u64,
    sample_rate: f64,
    frame_count: u32,
    pitch_output: *mut f64,
    formant_output: *mut f64,
    matched_indices_output: *mut i64,
    previous_indices_output: *mut i64,
) {
    if frame_count == 0 {
        return;
    }
    if pitch_output.is_null()
        || formant_output.is_null()
        || matched_indices_output.is_null()
        || previous_indices_output.is_null()
    {
        return;
    }
    let frame_count = frame_count as usize;
    let pitches = slice::from_raw_parts_mut(pitch_output, frame_count);
    let formants = slice::from_raw_parts_mut(formant_output, frame_count);
    let matched_indices = slice::from_raw_parts_mut(matched_indices_output, frame_count);
    let previous_indices = slice::from_raw_parts_mut(previous_indices_output, frame_count);
    pitches.fill(0.0);
    formants.fill(0.0);
    matched_indices.fill(-1);
    previous_indices.fill(-1);
    if ranges.is_null()
        || curves.is_null()
        || range_count == 0
        || curve_count < range_count
        || region_length == 0
    {
        return;
    }

    let ranges = slice::from_raw_parts(ranges, range_count);
    let curves = slice::from_raw_parts(curves, curve_count);
    let seconds_per_sample = 1.0 / sample_rate.max(1.0);
    let mut values = [0.0; 2];
    for frame in 0..frame_count {
        let region_sample = first_region_sample.saturating_add(frame as u64) % region_length;
        let seconds = region_sample as f64 * seconds_per_sample;
        let indices = find_segment_indices(ranges, seconds);
        let matched = indices[0];
        let previous = indices[1];
        matched_indices[frame] = matched;
        previous_indices[frame] = previous;
        if matched < 0 {
            continue;
        }
        let curve = &curves[matched as usize];
        hirari_audio_note_curve_at(
            curve.anchors,
            curve.anchor_count,
            seconds,
            curve.pitch_offset_cents,
            curve.formant_offset_cents,
            values.as_mut_ptr(),
        );
        pitches[frame] = values[0];
        formants[frame] = values[1];
    }
}

#[inline]
fn ratio_for_cents(cents: f64) -> f64 {
    crate::dsp_limits::cents_to_pitch_ratio(cents)
}

#[inline]
fn integrate_linear_pitch(left: f64, right: f64, seconds: f64, offset: f64) -> f64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0.0;
    }
    let limit = crate::dsp_limits::MAX_PITCH_CENTS;
    let left = (offset + left).clamp(-limit, limit);
    let right = (offset + right).clamp(-limit, limit);
    let delta = right - left;
    const CENTS_TO_EXPONENT: f64 = 0.000_577_622_650_466_621_1;
    if delta.abs() < 1.0e-8 {
        return seconds * ratio_for_cents(left);
    }
    let exponent = CENTS_TO_EXPONENT * delta;
    let integral = seconds * ratio_for_cents(left) * exponent.exp_m1() / exponent;
    if integral.is_finite() {
        integral
    } else {
        seconds * ratio_for_cents(left)
    }
}

/// Applies the affine timing transform to a segment's anchor points.
#[path = "audio_note_curve_warp.rs"]
mod warp;
pub use warp::*;

#[cfg(test)]
#[path = "audio_note_curve_edit_tests.rs"]
mod edit_tests;


#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "audio_note_curve_differential_tests.rs"]
mod differential_tests;
