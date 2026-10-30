use crate::audio_note_curve::{
    HirariAudioNoteAnchor, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
};

fn ratio_for_cents(cents: f64) -> f64 {
    (cents.clamp(-4800.0, 4800.0) / 1200.0).exp2()
}

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

unsafe fn interpolate(
    anchors: *const HirariAudioNoteAnchor,
    count: usize,
    seconds: f64,
    base: f64,
    formant: bool,
) -> f64 {
    if !seconds.is_finite() {
        return 0.0;
    }
    if count == 0 {
        return base;
    }
    let value = |anchor: &HirariAudioNoteAnchor| {
        if formant {
            anchor.formant_cents
        } else {
            anchor.pitch_cents
        }
    };
    if seconds <= (*anchors).position_seconds {
        return base + value(&*anchors);
    }
    if seconds >= (*anchors.add(count - 1)).position_seconds {
        return base + value(&*anchors.add(count - 1));
    }
    let mut upper = 0usize;
    while upper < count && (*anchors.add(upper)).position_seconds <= seconds {
        upper += 1;
    }
    let left = &*anchors.add(upper - 1);
    let right = &*anchors.add(upper);
    let span = right.position_seconds - left.position_seconds;
    let t = if span > 0.0 {
        ((seconds - left.position_seconds) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (l, r) = (value(left), value(right));
    base + l + (r - l) * t
}

#[allow(clippy::too_many_arguments)]
unsafe fn integral_at(
    anchors: *const HirariAudioNoteAnchor,
    count: usize,
    prefix: *const f64,
    start: f64,
    finish: f64,
    pitch_offset: f64,
    seconds: f64,
) -> f64 {
    if !seconds.is_finite() || seconds <= start {
        return 0.0;
    }
    let end = seconds.min(finish);
    if end <= start {
        return 0.0;
    }
    if count == 0 {
        return (end - start) * ratio_for_cents(pitch_offset);
    }
    let first = &*anchors;
    let mut total = (end.min(first.position_seconds) - start)
        * ratio_for_cents(pitch_offset + first.pitch_cents);
    if end <= first.position_seconds {
        return total;
    }
    let mut upper = 0usize;
    while upper < count && (*anchors.add(upper)).position_seconds <= end {
        upper += 1;
    }
    let left_index = upper - 1;
    total += *prefix.add(left_index);
    if upper == count {
        let last = &*anchors.add(count - 1);
        total += (end - last.position_seconds) * ratio_for_cents(pitch_offset + last.pitch_cents);
    } else {
        let left = &*anchors.add(left_index);
        let right = &*anchors.add(upper);
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
            pitch_offset,
        );
    }
    if total.is_finite() {
        total
    } else {
        0.0
    }
}

#[no_mangle]
pub unsafe extern "C" fn audio_note_curve_reference_build_prefix(
    anchors: *const HirariAudioNoteAnchor,
    count: usize,
    pitch_offset: f64,
    output: *mut f64,
) {
    if count == 0 || anchors.is_null() || output.is_null() {
        return;
    }
    *output = 0.0;
    for index in 1..count {
        let previous = &*anchors.add(index - 1);
        let current = &*anchors.add(index);
        *output.add(index) = *output.add(index - 1)
            + integrate_linear_pitch(
                previous.pitch_cents,
                current.pitch_cents,
                current.position_seconds - previous.position_seconds,
                pitch_offset,
            );
    }
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn audio_note_curve_reference_evaluate(
    anchors: *const HirariAudioNoteAnchor,
    count: usize,
    prefix: *const f64,
    start: f64,
    finish: f64,
    pitch_offset: f64,
    formant_offset: f64,
    seconds: f64,
    output: *mut f64,
) {
    if output.is_null() {
        return;
    }
    *output = interpolate(anchors, count, seconds, pitch_offset, false);
    *output.add(1) = interpolate(anchors, count, seconds, formant_offset, true);
    *output.add(2) = integral_at(anchors, count, prefix, start, finish, pitch_offset, seconds);
}

#[no_mangle]
pub unsafe extern "C" fn audio_note_segment_lookup_reference(
    ranges: *const HirariAudioNoteSegmentRange,
    count: usize,
    seconds: f64,
    output: *mut i64,
) {
    if output.is_null() {
        return;
    }
    *output = -1;
    *output.add(1) = -1;
    *output.add(2) = -1;
    if ranges.is_null() || count == 0 || !seconds.is_finite() {
        return;
    }
    let mut upper = 0usize;
    while upper < count && (*ranges.add(upper)).start_seconds <= seconds {
        upper += 1;
    }
    if upper == 0 {
        return;
    }
    let latest = upper - 1;
    if (*ranges.add(latest)).end_seconds < seconds {
        *output.add(1) = latest as i64;
    }
    if seconds <= (*ranges.add(latest)).end_seconds {
        *output.add(2) = latest as i64;
    }
    let mut cursor = upper;
    while cursor > 0 {
        let index = cursor - 1;
        let range = &*ranges.add(index);
        if range.end_seconds < seconds {
            break;
        }
        if seconds >= range.start_seconds && seconds <= range.end_seconds {
            *output = index as i64;
        }
        cursor = index;
    }
}

#[no_mangle]
pub unsafe extern "C" fn audio_note_phase_correction_reference(
    seconds: f64,
    matched: *const HirariAudioNoteCurveView,
    previous: *const HirariAudioNoteCurveView,
) -> f64 {
    if !matched.is_null() {
        let matched = &*matched;
        let segment_offset = (seconds - matched.start_seconds).max(0.0);
        let integral = integral_at(
            matched.anchors,
            matched.anchor_count,
            matched.integral_prefix,
            matched.start_seconds,
            matched.end_seconds,
            matched.pitch_offset_cents,
            seconds,
        );
        return matched.correction_before_seconds + integral - segment_offset;
    }
    if !previous.is_null() {
        let previous = &*previous;
        let integral = integral_at(
            previous.anchors,
            previous.anchor_count,
            previous.integral_prefix,
            previous.start_seconds,
            previous.end_seconds,
            previous.pitch_offset_cents,
            previous.end_seconds,
        );
        return previous.correction_before_seconds + integral
            - (previous.end_seconds - previous.start_seconds);
    }
    0.0
}
