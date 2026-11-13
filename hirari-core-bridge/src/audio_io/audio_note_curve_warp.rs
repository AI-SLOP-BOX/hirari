//! Warp-anchor/upsert/integral machinery for the note curve API, split out
//! of audio_note_curve.rs.
use super::*;

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
