use crate::region_processing::{NativeCompRange, NativeRangeEdit};

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_region_gain_reference(
    output: *mut f32,
    frames: u32,
    region_offset: u64,
    region_length: u64,
    timeline_length: u64,
    fade_in: u64,
    fade_out: u64,
    crossfade_in: u64,
    crossfade_out: u64,
    clip_gain: f32,
    comp_managed: u8,
    comp_ranges: *const NativeCompRange,
    comp_range_count: usize,
    range_edits: *const NativeRangeEdit,
    range_edit_count: usize,
) {
    if output.is_null() || frames == 0 || region_length == 0 || !clip_gain.is_finite() {
        return;
    }
    for index in 0..frames as usize {
        let relative = region_offset + index as u64;
        let loop_relative = relative % region_length;
        let mut comp_range: Option<&NativeCompRange> = None;
        if comp_managed != 0 {
            let mut low = 0usize;
            let mut high = comp_range_count;
            while low < high {
                let middle = low + (high - low) / 2;
                if (*comp_ranges.add(middle)).start <= loop_relative {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            if low == 0 {
                *output.add(index) = 0.0;
                continue;
            }
            let candidate = &*comp_ranges.add(low - 1);
            if loop_relative < candidate.start || loop_relative >= candidate.end {
                *output.add(index) = 0.0;
                continue;
            }
            comp_range = Some(candidate);
        }

        let mut fade = 1.0f32;
        if fade_in > 0 && relative < fade_in {
            fade = relative as f32 / fade_in as f32;
        }
        if fade_out > 0 && timeline_length > fade_out && relative >= timeline_length.wrapping_sub(fade_out) {
            fade = fade.min((timeline_length.wrapping_sub(relative)) as f32 / fade_out as f32);
        }
        if crossfade_in > 0 && relative < crossfade_in {
            fade = fade.min(relative as f32 / crossfade_in as f32);
        }
        if crossfade_out > 0 && relative >= timeline_length.wrapping_sub(crossfade_out) {
            fade = fade.min((timeline_length.wrapping_sub(relative)) as f32 / crossfade_out as f32);
        }

        let mut range_gain = 1.0f32;
        let mut range_fade = 1.0f32;
        let mut range_muted = false;
        for edit_index in 0..range_edit_count {
            let edit = &*range_edits.add(edit_index);
            if edit.start > loop_relative {
                break;
            }
            if edit.start >= edit.end
                || loop_relative < edit.start
                || loop_relative >= edit.end
                || !edit.gain.is_finite()
                || edit.gain < 0.0
                || edit.gain > 16.0
            {
                continue;
            }
            range_gain = (range_gain * edit.gain).min(16.0);
            if !range_gain.is_finite() {
                range_gain = 16.0;
            }
            let length = edit.end - edit.start;
            if edit.fade_in > 0
                && edit.fade_in <= length
                && loop_relative.wrapping_sub(edit.start) < edit.fade_in
            {
                range_fade = range_fade
                    .min((loop_relative.wrapping_sub(edit.start)) as f32 / edit.fade_in as f32);
            }
            if edit.fade_out > 0
                && edit.fade_out <= length
                && loop_relative.wrapping_sub(edit.start) >= length - edit.fade_out
            {
                range_fade =
                    range_fade.min((edit.end.wrapping_sub(loop_relative)) as f32 / edit.fade_out as f32);
            }
            if edit.gain == 0.0 {
                range_muted = true;
            }
        }
        if range_muted {
            *output.add(index) = 0.0;
            continue;
        }
        if !range_fade.is_finite() {
            range_fade = 1.0;
        }
        let mut comp_fade = 1.0f32;
        if let Some(comp_range) = comp_range {
            if comp_range.fade_in > 0 && loop_relative - comp_range.start < comp_range.fade_in {
                let phase =
                    (loop_relative - comp_range.start + 1) as f32 / comp_range.fade_in as f32;
                comp_fade = (phase.clamp(0.0, 1.0) * 1.570_796_4).sin();
            }
            if comp_range.fade_out > 0 && comp_range.end.wrapping_sub(loop_relative) <= comp_range.fade_out {
                let phase = (comp_range.end.wrapping_sub(loop_relative)) as f32 / comp_range.fade_out as f32;
                comp_fade = comp_fade.min((phase.clamp(0.0, 1.0) * 1.570_796_4).sin());
            }
        }
        *output.add(index) =
            clip_gain * (fade * range_fade * range_gain * comp_fade).clamp(0.0, 16.0);
    }
}
