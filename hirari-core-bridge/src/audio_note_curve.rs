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
    2.0f64.powf(cents.clamp(-4800.0, 4800.0) / 1200.0)
}

#[inline]
fn integrate_linear_pitch(left: f64, right: f64, seconds: f64, offset: f64) -> f64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0.0;
    }
    let left = (offset + left).clamp(-4800.0, 4800.0);
    let right = (offset + right).clamp(-4800.0, 4800.0);
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
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_warp_anchors(
    anchors: *mut HirariAudioNoteAnchor,
    anchor_count: usize,
    old_start: f64,
    old_end: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    let old_span = old_end - old_start;
    let new_span = new_end - new_start;
    if !old_start.is_finite()
        || !old_end.is_finite()
        || !new_start.is_finite()
        || !new_end.is_finite()
        || old_span <= 0.0
        || new_span <= 0.0
        || !old_span.is_finite()
        || !new_span.is_finite()
        || (anchor_count != 0 && anchors.is_null())
    {
        return false;
    }
    let scale = new_span / old_span;
    if !scale.is_finite() {
        return false;
    }
    if anchor_count == 0 {
        return true;
    }
    for anchor in slice::from_raw_parts_mut(anchors, anchor_count) {
        anchor.position_seconds = new_start + (anchor.position_seconds - old_start) * scale;
    }
    true
}

/// Inserts or replaces one anchor while keeping the segment array ordered.
/// Returns usize::MAX when the request is invalid or the output has no room.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_upsert_anchor(
    anchors: *mut HirariAudioNoteAnchor,
    anchor_count: usize,
    capacity: usize,
    segment_start: f64,
    segment_end: f64,
    position_seconds: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> usize {
    if !segment_start.is_finite()
        || !segment_end.is_finite()
        || segment_end <= segment_start
        || !position_seconds.is_finite()
        || !pitch_cents.is_finite()
        || !formant_cents.is_finite()
        || position_seconds < segment_start
        || position_seconds > segment_end
        || anchor_count > capacity
        || (capacity != 0 && anchors.is_null())
    {
        return usize::MAX;
    }
    if anchor_count == 0 {
        if capacity == 0 {
            return usize::MAX;
        }
        slice::from_raw_parts_mut(anchors, capacity)[0] = HirariAudioNoteAnchor {
            position_seconds,
            pitch_cents,
            formant_cents,
        };
        return 1;
    }
    let values = slice::from_raw_parts_mut(anchors, capacity);
    let index =
        values[..anchor_count].partition_point(|anchor| anchor.position_seconds < position_seconds);
    if index < anchor_count && (values[index].position_seconds - position_seconds).abs() < 1e-9 {
        values[index] = HirariAudioNoteAnchor {
            position_seconds,
            pitch_cents,
            formant_cents,
        };
        return anchor_count;
    }
    if anchor_count == capacity {
        return usize::MAX;
    }
    values.copy_within(index..anchor_count, index + 1);
    values[index] = HirariAudioNoteAnchor {
        position_seconds,
        pitch_cents,
        formant_cents,
    };
    anchor_count + 1
}

/// Moves a pitch or formant anchor and reorders the curve atomically.
/// Returns the resulting anchor count, or usize::MAX when no match exists.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_move_anchor(
    anchors: *mut HirariAudioNoteAnchor,
    anchor_count: usize,
    segment_start: f64,
    segment_end: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    edit_formant: u8,
    pitch_offset: f64,
    formant_offset: f64,
) -> usize {
    let max_cents = if edit_formant != 0 { 2400.0 } else { 4800.0 };
    if anchor_count == 0
        || anchors.is_null()
        || !segment_start.is_finite()
        || !segment_end.is_finite()
        || segment_end <= segment_start
        || !old_position.is_finite()
        || !new_position.is_finite()
        || new_position < segment_start
        || new_position > segment_end
        || !value_cents.is_finite()
        || value_cents.abs() > max_cents
        || !pitch_offset.is_finite()
        || !formant_offset.is_finite()
    {
        return usize::MAX;
    }

    let source = slice::from_raw_parts(anchors, anchor_count);
    let tolerance = 1.0e-6_f64.max(old_position.abs() * 2.0e-7);
    let Some(index) = source
        .iter()
        .position(|anchor| (anchor.position_seconds - old_position).abs() < tolerance)
    else {
        return usize::MAX;
    };
    let mut moved = source[index];
    moved.position_seconds = new_position;
    if edit_formant != 0 {
        moved.formant_cents = value_cents - formant_offset;
    } else {
        moved.pitch_cents = value_cents - pitch_offset;
    }
    if !moved.position_seconds.is_finite()
        || !moved.pitch_cents.is_finite()
        || !moved.formant_cents.is_finite()
    {
        return usize::MAX;
    }

    let mut result = source.to_vec();
    result.remove(index);
    let insertion =
        result.partition_point(|anchor| anchor.position_seconds < moved.position_seconds);
    if insertion < result.len()
        && (result[insertion].position_seconds - moved.position_seconds).abs() < 1.0e-9
    {
        result[insertion] = moved;
    } else {
        result.insert(insertion, moved);
    }
    let count = result.len();
    slice::from_raw_parts_mut(anchors, anchor_count)[..count].copy_from_slice(&result);
    count
}

/// Builds an anchor for a pitch-only or formant-only edit while preserving
/// the other curve's currently interpolated value at that time.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_component_anchor(
    anchors: *const HirariAudioNoteAnchor,
    anchor_count: usize,
    segment_start: f64,
    segment_end: f64,
    position_seconds: f64,
    value_cents: f64,
    edit_formant: u8,
    pitch_offset: f64,
    formant_offset: f64,
    output: *mut HirariAudioNoteAnchor,
) -> bool {
    let limit = if edit_formant != 0 { 2400.0 } else { 4800.0 };
    if output.is_null()
        || (anchor_count != 0 && anchors.is_null())
        || !segment_start.is_finite()
        || !segment_end.is_finite()
        || segment_end <= segment_start
        || !position_seconds.is_finite()
        || position_seconds < segment_start
        || position_seconds > segment_end
        || !value_cents.is_finite()
        || value_cents.abs() > limit
        || !pitch_offset.is_finite()
        || !formant_offset.is_finite()
    {
        return false;
    }
    let mut current = [0.0; 2];
    hirari_audio_note_curve_at(
        anchors,
        anchor_count,
        position_seconds,
        pitch_offset,
        formant_offset,
        current.as_mut_ptr(),
    );
    let anchor = if edit_formant != 0 {
        HirariAudioNoteAnchor {
            position_seconds,
            pitch_cents: current[0] - pitch_offset,
            formant_cents: value_cents - formant_offset,
        }
    } else {
        HirariAudioNoteAnchor {
            position_seconds,
            pitch_cents: value_cents - pitch_offset,
            formant_cents: current[1] - formant_offset,
        }
    };
    if !anchor.pitch_cents.is_finite() || !anchor.formant_cents.is_finite() {
        return false;
    }
    *output = anchor;
    true
}

/// Fills the prefix integrals used for random-access pitch correction.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_build_integral_prefix(
    anchors: *const HirariAudioNoteAnchor,
    anchor_count: usize,
    pitch_offset_cents: f64,
    prefix_output: *mut f64,
) {
    if anchor_count == 0 || anchors.is_null() || prefix_output.is_null() {
        return;
    }
    let anchors = slice::from_raw_parts(anchors, anchor_count);
    let prefix = slice::from_raw_parts_mut(prefix_output, anchor_count);
    prefix[0] = 0.0;
    for index in 1..anchor_count {
        prefix[index] = prefix[index - 1]
            + integrate_linear_pitch(
                anchors[index - 1].pitch_cents,
                anchors[index].pitch_cents,
                anchors[index].position_seconds - anchors[index - 1].position_seconds,
                pitch_offset_cents,
            );
    }
}

/// Evaluates both edited pitch and formant curves at one time position.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_at(
    anchors: *const HirariAudioNoteAnchor,
    anchor_count: usize,
    seconds: f64,
    pitch_offset_cents: f64,
    formant_offset_cents: f64,
    output_pitch_formant: *mut f64,
) {
    if output_pitch_formant.is_null() {
        return;
    }
    let output = slice::from_raw_parts_mut(output_pitch_formant, 2);
    output[0] = 0.0;
    output[1] = 0.0;
    if !seconds.is_finite() {
        return;
    }
    if anchor_count == 0 {
        output[0] = pitch_offset_cents;
        output[1] = formant_offset_cents;
        return;
    }
    if anchors.is_null() {
        return;
    }
    let anchors = slice::from_raw_parts(anchors, anchor_count);
    let upper = anchors.partition_point(|anchor| anchor.position_seconds <= seconds);
    let (left, right) = if upper == 0 {
        (&anchors[0], &anchors[0])
    } else if upper >= anchor_count {
        (&anchors[anchor_count - 1], &anchors[anchor_count - 1])
    } else {
        (&anchors[upper - 1], &anchors[upper])
    };
    let t = if left.position_seconds == right.position_seconds {
        0.0
    } else {
        ((seconds - left.position_seconds) / (right.position_seconds - left.position_seconds))
            .clamp(0.0, 1.0)
    };
    output[0] = pitch_offset_cents + left.pitch_cents + (right.pitch_cents - left.pitch_cents) * t;
    output[1] =
        formant_offset_cents + left.formant_cents + (right.formant_cents - left.formant_cents) * t;
}

/// Integrates the pitch ratio from segment start to `seconds` using the
/// control-thread prefix array and exact exponential integration per glide.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_curve_integral_at(
    anchors: *const HirariAudioNoteAnchor,
    anchor_count: usize,
    prefix: *const f64,
    start_seconds: f64,
    end_seconds: f64,
    pitch_offset_cents: f64,
    seconds: f64,
) -> f64 {
    if !seconds.is_finite() || seconds <= start_seconds {
        return 0.0;
    }
    let end = seconds.min(end_seconds);
    if end <= start_seconds {
        return 0.0;
    }
    if anchor_count == 0 {
        return (end - start_seconds) * ratio_for_cents(pitch_offset_cents);
    }
    if anchors.is_null() || prefix.is_null() {
        return 0.0;
    }
    let anchors = slice::from_raw_parts(anchors, anchor_count);
    let prefix = slice::from_raw_parts(prefix, anchor_count);
    let first = &anchors[0];
    let mut total = (end.min(first.position_seconds) - start_seconds)
        * ratio_for_cents(pitch_offset_cents + first.pitch_cents);
    if end <= first.position_seconds {
        return total;
    }
    let upper = anchors.partition_point(|anchor| anchor.position_seconds <= end);
    let left_index = upper.saturating_sub(1);
    total += prefix[left_index];
    if upper >= anchor_count {
        total += (end - anchors[anchor_count - 1].position_seconds)
            * ratio_for_cents(pitch_offset_cents + anchors[anchor_count - 1].pitch_cents);
    } else {
        let left = &anchors[left_index];
        let right = &anchors[upper];
        let span = right.position_seconds - left.position_seconds;
        let t = if span > 0.0 {
            ((end - left.position_seconds) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let end_cents = left.pitch_cents + (right.pitch_cents - left.pitch_cents) * t;
        total += integrate_linear_pitch(
            left.pitch_cents,
            end_cents,
            end - left.position_seconds,
            pitch_offset_cents,
        );
    }
    if total.is_finite() {
        total
    } else {
        0.0
    }
}

/// Sorts note segments by start time and calculates the accumulated phase
/// correction before each one. The caller applies the returned permutation to
/// its owned segment objects and stores the matching corrections.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_note_rebuild_phase_prefixes(
    segments: *const HirariAudioNotePhaseView,
    segment_count: usize,
    ordered_indices: *mut u32,
    corrections: *mut f64,
) -> bool {
    if segment_count > 0
        && (segments.is_null() || ordered_indices.is_null() || corrections.is_null())
    {
        return false;
    }
    if segment_count == 0 {
        return true;
    }
    let segments = slice::from_raw_parts(segments, segment_count);
    let indices = slice::from_raw_parts_mut(ordered_indices, segment_count);
    let corrections = slice::from_raw_parts_mut(corrections, segment_count);
    for (index, _) in segments.iter().enumerate() {
        indices[index] = index as u32;
    }
    indices.sort_by(|left, right| {
        segments[*left as usize]
            .start_seconds
            .total_cmp(&segments[*right as usize].start_seconds)
            .then_with(|| left.cmp(right))
    });
    let mut accumulated = 0.0;
    for (position, index) in indices.iter().copied().enumerate() {
        let segment = &segments[index as usize];
        corrections[position] = accumulated;
        let pitch_integral = hirari_audio_note_curve_integral_at(
            segment.anchors,
            segment.anchor_count,
            segment.integral_prefix,
            segment.start_seconds,
            segment.end_seconds,
            segment.pitch_offset_cents,
            segment.end_seconds,
        );
        accumulated += pitch_integral - (segment.end_seconds - segment.start_seconds);
    }
    true
}

#[cfg(test)]
mod edit_tests {
    use super::{
        hirari_audio_note_can_warp_segment, hirari_audio_note_curve_component_anchor,
        hirari_audio_note_curve_move_anchor, hirari_audio_note_curve_upsert_anchor,
        hirari_audio_note_curve_warp_anchors, hirari_audio_note_find_segment_for_edit,
        hirari_audio_note_rebuild_phase_prefixes, hirari_audio_note_upsert_segment_slot,
        hirari_audio_note_validate_segments, HirariAudioNoteAnchor, HirariAudioNotePhaseView,
        HirariAudioNoteSegmentValidation,
    };

    #[test]
    fn segment_batch_validation_sorts_and_rejects_invalid_batches() {
        let anchors = [HirariAudioNoteAnchor {
            position_seconds: 1.0,
            pitch_cents: 0.0,
            formant_cents: 0.0,
        }];
        let segments = [
            HirariAudioNoteSegmentValidation {
                start_seconds: 4.0,
                end_seconds: 6.0,
                detected_pitch_cents: 100.0,
                ..Default::default()
            },
            HirariAudioNoteSegmentValidation {
                start_seconds: 0.0,
                end_seconds: 2.0,
                detected_pitch_cents: 90.0,
                anchors: anchors.as_ptr(),
                anchor_count: anchors.len(),
                ..Default::default()
            },
        ];
        let mut order = [u32::MAX; 2];
        assert!(unsafe {
            hirari_audio_note_validate_segments(
                segments.as_ptr(),
                segments.len(),
                order.as_mut_ptr(),
            )
        });
        assert_eq!(order, [1, 0]);

        let overlapping = [
            segments[1],
            HirariAudioNoteSegmentValidation {
                start_seconds: 1.5,
                end_seconds: 3.0,
                ..segments[1]
            },
        ];
        assert!(!unsafe {
            hirari_audio_note_validate_segments(
                overlapping.as_ptr(),
                overlapping.len(),
                order.as_mut_ptr(),
            )
        });

        let invalid_anchor = [HirariAudioNoteAnchor {
            position_seconds: 2.5,
            ..anchors[0]
        }];
        let invalid = [HirariAudioNoteSegmentValidation {
            anchors: invalid_anchor.as_ptr(),
            ..segments[1]
        }];
        assert!(!unsafe {
            hirari_audio_note_validate_segments(invalid.as_ptr(), invalid.len(), order.as_mut_ptr())
        });
    }

    #[test]
    fn phase_prefix_rebuild_sorts_and_accumulates_pitch_ratio_delta() {
        let segments = [
            HirariAudioNotePhaseView {
                start_seconds: 3.0,
                end_seconds: 4.0,
                ..Default::default()
            },
            HirariAudioNotePhaseView {
                start_seconds: 0.0,
                end_seconds: 1.0,
                pitch_offset_cents: 1200.0,
                ..Default::default()
            },
        ];
        let mut order = [u32::MAX; 2];
        let mut corrections = [f64::NAN; 2];
        assert!(unsafe {
            hirari_audio_note_rebuild_phase_prefixes(
                segments.as_ptr(),
                segments.len(),
                order.as_mut_ptr(),
                corrections.as_mut_ptr(),
            )
        });
        assert_eq!(order, [1, 0]);
        assert_eq!(corrections, [0.0, 1.0]);
    }

    #[test]
    fn segment_edit_rules_match_existing_overlap_and_start_tolerance() {
        let ranges = [
            super::HirariAudioNoteSegmentRange {
                start_seconds: 1.0,
                end_seconds: 2.0,
                detected_pitch_cents: 0.0,
            },
            super::HirariAudioNoteSegmentRange {
                start_seconds: 3.0,
                end_seconds: 4.0,
                detected_pitch_cents: 0.0,
            },
        ];
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 1.0, 1.5) },
            0
        );
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 1.5, 2.5) },
            -2
        );
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 2.0, 3.0) },
            -1
        );
        assert_eq!(
            unsafe { hirari_audio_note_find_segment_for_edit(ranges.as_ptr(), 2, 1.0 + 5.0e-10) },
            0
        );
        assert!(unsafe { hirari_audio_note_can_warp_segment(ranges.as_ptr(), 2, 0, 0.5, 2.5) });
        assert!(!unsafe { hirari_audio_note_can_warp_segment(ranges.as_ptr(), 2, 0, 1.5, 3.5) });
    }

    #[test]
    fn anchor_upsert_inserts_in_order_and_replaces_nearby_points() {
        let mut anchors = [HirariAudioNoteAnchor::default(); 4];
        anchors[0] = HirariAudioNoteAnchor {
            position_seconds: 1.0,
            pitch_cents: 10.0,
            formant_cents: 20.0,
        };
        anchors[1] = HirariAudioNoteAnchor {
            position_seconds: 3.0,
            pitch_cents: 30.0,
            formant_cents: 40.0,
        };
        let count = unsafe {
            hirari_audio_note_curve_upsert_anchor(
                anchors.as_mut_ptr(),
                2,
                4,
                0.0,
                4.0,
                2.0,
                50.0,
                60.0,
            )
        };
        assert_eq!(count, 3);
        assert_eq!(
            anchors[..count]
                .iter()
                .map(|a| a.position_seconds)
                .collect::<Vec<_>>(),
            [1.0, 2.0, 3.0]
        );

        let count = unsafe {
            hirari_audio_note_curve_upsert_anchor(
                anchors.as_mut_ptr(),
                count,
                4,
                0.0,
                4.0,
                2.0 - 5.0e-10,
                70.0,
                80.0,
            )
        };
        assert_eq!(count, 3);
        assert_eq!(anchors[1].pitch_cents, 70.0);
        assert_eq!(anchors[1].formant_cents, 80.0);
    }

    #[test]
    fn anchor_warp_preserves_relative_positions_and_rejects_invalid_ranges() {
        let mut anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 2.0,
                pitch_cents: 1.0,
                formant_cents: 2.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 4.0,
                pitch_cents: 3.0,
                formant_cents: 4.0,
            },
        ];
        assert!(unsafe {
            hirari_audio_note_curve_warp_anchors(anchors.as_mut_ptr(), 2, 1.0, 5.0, 10.0, 16.0)
        });
        assert_eq!(anchors[0].position_seconds, 11.5);
        assert_eq!(anchors[1].position_seconds, 14.5);
        assert!(!unsafe {
            hirari_audio_note_curve_warp_anchors(anchors.as_mut_ptr(), 2, 1.0, 1.0, 0.0, 2.0)
        });
        assert_eq!(anchors[0].position_seconds, 11.5);
    }

    #[test]
    fn moving_an_anchor_reorders_or_merges_and_keeps_failure_atomic() {
        let mut anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 1.0,
                pitch_cents: 10.0,
                formant_cents: 11.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 3.0,
                pitch_cents: 30.0,
                formant_cents: 31.0,
            },
        ];
        let count = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                2,
                0.0,
                4.0,
                1.0,
                2.0,
                150.0,
                0,
                25.0,
                5.0,
            )
        };
        assert_eq!(count, 2);
        assert_eq!(anchors[0].position_seconds, 2.0);
        assert_eq!(anchors[0].pitch_cents, 125.0);

        let count = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                3.0,
                240.0,
                1,
                25.0,
                40.0,
            )
        };
        assert_eq!(count, 1);
        assert_eq!(anchors[0].position_seconds, 3.0);
        assert_eq!(anchors[0].formant_cents, 200.0);
        let before = anchors[0];
        let failed = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                1,
                0.0,
                4.0,
                1.5,
                2.0,
                100.0,
                0,
                0.0,
                0.0,
            )
        };
        assert_eq!(failed, usize::MAX);
        assert_eq!(anchors[0].position_seconds, before.position_seconds);
        assert_eq!(anchors[0].pitch_cents, before.pitch_cents);
        assert_eq!(anchors[0].formant_cents, before.formant_cents);
    }

    #[test]
    fn component_edit_preserves_the_other_interpolated_curve() {
        let anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 1.0,
                pitch_cents: 100.0,
                formant_cents: -20.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 3.0,
                pitch_cents: 300.0,
                formant_cents: 20.0,
            },
        ];
        let mut output = HirariAudioNoteAnchor::default();
        assert!(unsafe {
            hirari_audio_note_curve_component_anchor(
                anchors.as_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                500.0,
                0,
                10.0,
                5.0,
                &mut output,
            )
        });
        assert_eq!(output.pitch_cents, 490.0);
        assert_eq!(output.formant_cents, 0.0);

        assert!(unsafe {
            hirari_audio_note_curve_component_anchor(
                anchors.as_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                100.0,
                1,
                10.0,
                5.0,
                &mut output,
            )
        });
        assert_eq!(output.pitch_cents, 200.0);
        assert_eq!(output.formant_cents, 95.0);
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "audio_note_curve_differential_tests.rs"]
mod differential_tests;
