use crate::audio_note_curve::{
    hirari_audio_note_curve_block, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
};
use crate::region_warp::{
    hirari_region_source_block, hirari_region_source_position_at, HirariWarpMarker,
};
use std::ptr;
use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeRangeEdit {
    pub start: u64,
    pub end: u64,
    pub gain: f32,
    pub fade_in: u64,
    pub fade_out: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeRegionGeometry {
    pub start: u64,
    pub length: u64,
    pub source_length: u64,
    pub source_offset: u64,
    pub base_start: u64,
    pub base_source_offset: u64,
    pub base_length: u64,
    pub loop_count: u32,
    pub fade_in: u64,
    pub fade_out: u64,
    pub warp_ratio: f64,
    pub source_sample_rate: f64,
    pub timeline_sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeRegionAddMetadata {
    pub start: u64,
    pub length: u64,
    pub source_length: u64,
    pub source_offset: u64,
    pub base_start: u64,
    pub base_source_offset: u64,
    pub base_length: u64,
    pub loop_count: u32,
    pub clip_gain: f32,
    pub warp_ratio: f64,
    pub pitch_semitones: f32,
}

/// Applies the scalar invariants for a newly inserted region. Audio ownership
/// stays in C++; all shared range and overflow policy lives in the Rust core.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_normalize_add_metadata(
    metadata: *mut NativeRegionAddMetadata,
    has_audio: bool,
    audio_samples: u64,
    audio_channels: u32,
) -> bool {
    let Some(metadata) = metadata.as_mut() else {
        return false;
    };
    if metadata.length == 0 || metadata.start.checked_add(metadata.length).is_none() {
        return false;
    }
    if !metadata.warp_ratio.is_finite() || !(0.5..=2.0).contains(&metadata.warp_ratio) {
        metadata.warp_ratio = 1.0;
    }
    if !metadata.pitch_semitones.is_finite() || !(-24.0..=24.0).contains(&metadata.pitch_semitones)
    {
        metadata.pitch_semitones = 0.0;
    }
    if metadata.loop_count == 0
        || metadata.loop_count > 1024
        || metadata.length > u64::MAX / u64::from(metadata.loop_count)
    {
        metadata.loop_count = 1;
    }
    let Some(timeline_length) = metadata.length.checked_mul(u64::from(metadata.loop_count)) else {
        return false;
    };
    if metadata.start.checked_add(timeline_length).is_none() {
        return false;
    }
    if !metadata.clip_gain.is_finite() {
        metadata.clip_gain = 1.0;
    }
    metadata.clip_gain = metadata.clip_gain.clamp(0.0, 2.0);

    if has_audio {
        if audio_channels == 0 || audio_samples == 0 || metadata.source_offset >= audio_samples {
            return false;
        }
        let available = audio_samples - metadata.source_offset;
        metadata.source_length = if metadata.source_length == 0 {
            available
        } else {
            metadata.source_length.min(available)
        };
        if metadata.source_length == 0 {
            return false;
        }
        if metadata.base_length == 0 {
            metadata.base_length = metadata.source_length;
        }
        if metadata.base_start == 0 {
            metadata.base_start = metadata.start;
        }
        if metadata.base_source_offset == 0 {
            metadata.base_source_offset = metadata.source_offset;
        }
    } else if metadata.source_length == 0 {
        metadata.source_length = metadata.length;
    }

    metadata.start.checked_add(metadata.length).is_some()
}

fn source_frames_per_timeline_frame(region: &NativeRegionGeometry) -> Option<f64> {
    let source_rate = if region.source_sample_rate.is_finite() && region.source_sample_rate > 0.0 {
        region.source_sample_rate
    } else {
        region.timeline_sample_rate
    };
    let timeline_rate =
        if region.timeline_sample_rate.is_finite() && region.timeline_sample_rate > 0.0 {
            region.timeline_sample_rate
        } else {
            source_rate
        };
    let conversion = source_rate / timeline_rate;
    let effective = region.warp_ratio * conversion;
    (effective.is_finite() && effective > 0.0)
        .then_some(effective)
        .or_else(|| {
            (region.warp_ratio.is_finite() && region.warp_ratio > 0.0).then_some(region.warp_ratio)
        })
}

/// Computes the timeline geometry for a warp-ratio edit. C++ retains the
/// Region/audio ownership and applies this validated result under its edit lock.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_set_warp_ratio_geometry(
    region: *const NativeRegionGeometry,
    ratio: f64,
    output: *mut NativeRegionGeometry,
) -> bool {
    let (Some(region), Some(output)) = (region.as_ref(), output.as_mut()) else {
        return false;
    };
    if !ratio.is_finite()
        || !(0.5..=2.0).contains(&ratio)
        || region.loop_count == 0
        || region.source_offset < region.base_source_offset
    {
        return false;
    }
    let source_length = if region.source_length == 0 {
        region.length
    } else {
        region.source_length
    };
    let conversion = if region.warp_ratio > 0.0 {
        let Some(old_source_per_timeline) = source_frames_per_timeline_frame(region) else {
            return false;
        };
        old_source_per_timeline / region.warp_ratio
    } else {
        1.0
    };
    let source_per_timeline = ratio * conversion;
    if !source_per_timeline.is_finite() || source_per_timeline <= 0.0 {
        return false;
    }
    let duration = (source_length as f64 / source_per_timeline).ceil();
    let timeline_trim =
        ((region.source_offset - region.base_source_offset) as f64 / source_per_timeline).ceil();
    if !duration.is_finite()
        || duration < 1.0
        || duration >= u64::MAX as f64
        || !timeline_trim.is_finite()
        || timeline_trim < 0.0
        || timeline_trim >= u64::MAX as f64
    {
        return false;
    }
    let length = duration as u64;
    let trim = timeline_trim as u64;
    let Some(start) = region.base_start.checked_add(trim) else {
        return false;
    };
    let Some(total_length) = length.checked_mul(u64::from(region.loop_count)) else {
        return false;
    };
    if start.checked_add(length).is_none() || start.checked_add(total_length).is_none() {
        return false;
    }
    *output = *region;
    output.start = start;
    output.length = length;
    output.fade_in = region.fade_in.min(length);
    output.fade_out = region.fade_out.min(length);
    output.warp_ratio = ratio;
    true
}

/// Computes source and timeline geometry for a normalized trim edit.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_set_trim_geometry(
    region: *const NativeRegionGeometry,
    start_normalized: f32,
    end_normalized: f32,
    output: *mut NativeRegionGeometry,
) -> bool {
    let (Some(region), Some(output)) = (region.as_ref(), output.as_mut()) else {
        return false;
    };
    if !start_normalized.is_finite() || !end_normalized.is_finite() || region.loop_count == 0 {
        return false;
    }
    let start_normalized = start_normalized.clamp(0.0, 1.0);
    let end_normalized = end_normalized.clamp(0.0, 1.0);
    if end_normalized <= start_normalized || region.source_offset < region.base_source_offset {
        return false;
    }
    let base_length = if region.base_length == 0 {
        region.length
    } else {
        region.base_length
    };
    if base_length == 0 {
        return false;
    }
    let offset = (base_length as f64 * f64::from(start_normalized)) as u64;
    let end = (base_length as f64 * f64::from(end_normalized)) as u64;
    if end <= offset {
        return false;
    }
    let Some(source_offset) = region.base_source_offset.checked_add(offset) else {
        return false;
    };
    let source_length = end - offset;
    let Some(source_per_timeline) = source_frames_per_timeline_frame(region) else {
        return false;
    };
    let timeline_offset = (offset as f64 / source_per_timeline).ceil();
    let length = (source_length as f64 / source_per_timeline).ceil();
    if !timeline_offset.is_finite()
        || timeline_offset < 0.0
        || timeline_offset >= u64::MAX as f64
        || !length.is_finite()
        || length < 1.0
        || length >= u64::MAX as f64
    {
        return false;
    }
    let timeline_offset = timeline_offset as u64;
    let length = length as u64;
    let Some(start) = region.base_start.checked_add(timeline_offset) else {
        return false;
    };
    let Some(total_length) = length.checked_mul(u64::from(region.loop_count)) else {
        return false;
    };
    if start.checked_add(length).is_none() || start.checked_add(total_length).is_none() {
        return false;
    }
    *output = *region;
    output.start = start;
    output.length = length;
    output.source_offset = source_offset;
    output.source_length = source_length;
    output.fade_in = region.fade_in.min(length);
    output.fade_out = region.fade_out.min(length);
    true
}

/// Repositions a trimmed region while retaining the source/timeline anchor
/// represented by its base start and source offset.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_move_geometry(
    region: *const NativeRegionGeometry,
    new_start: u64,
    output: *mut NativeRegionGeometry,
) -> bool {
    let (Some(region), Some(output)) = (region.as_ref(), output.as_mut()) else {
        return false;
    };
    let source_trim = region
        .source_offset
        .saturating_sub(region.base_source_offset);
    let Some(source_per_timeline) = source_frames_per_timeline_frame(region) else {
        return false;
    };
    let timeline_trim = (source_trim as f64 / source_per_timeline).ceil();
    if !timeline_trim.is_finite() || timeline_trim < 0.0 || timeline_trim > new_start as f64 {
        return false;
    }
    *output = *region;
    output.start = new_start;
    output.base_start = new_start.saturating_sub(timeline_trim as u64);
    true
}

/// Validates, clips, and orders a Track region's non-destructive range edits.
/// The realtime renderer consumes this normalized array without extra scans
/// for malformed project data.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_normalize_range_edits(
    edits: *mut NativeRangeEdit,
    count: usize,
    region_length: u64,
) -> usize {
    if count == 0 || edits.is_null() || region_length == 0 {
        return 0;
    }
    let bounded_count = count.min(4096);
    let edits = slice::from_raw_parts_mut(edits, bounded_count);
    let mut valid = Vec::with_capacity(bounded_count);
    for mut edit in edits.iter().copied() {
        if edit.start >= edit.end
            || edit.start >= region_length
            || !edit.gain.is_finite()
            || !(0.0..=16.0).contains(&edit.gain)
        {
            continue;
        }
        edit.end = edit.end.min(region_length);
        let length = edit.end - edit.start;
        edit.fade_in = edit.fade_in.min(length);
        edit.fade_out = edit.fade_out.min(length);
        valid.push(edit);
    }
    valid.sort_by_key(|edit| edit.start);
    let length = valid.len();
    edits[..length].copy_from_slice(&valid);
    length
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_range_edit_upsert(
    edits: *mut NativeRangeEdit,
    count: usize,
    capacity: usize,
    region_length: u64,
    start: u64,
    end: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
) -> usize {
    if edits.is_null()
        || count > capacity
        || count > 4096
        || start >= end
        || end > region_length
        || !gain.is_finite()
        || !(0.0..=16.0).contains(&gain)
        || fade_in > end - start
        || fade_out > end - start
    {
        return usize::MAX;
    }
    let edits = unsafe { slice::from_raw_parts_mut(edits, count) };
    let replacement = NativeRangeEdit {
        start,
        end,
        gain,
        fade_in,
        fade_out,
    };
    if let Some(existing) = edits
        .iter_mut()
        .find(|edit| edit.start == start && edit.end == end)
    {
        *existing = replacement;
        return count;
    }
    if count >= 4096 || count == capacity {
        return usize::MAX;
    }

    unsafe { edits.as_mut_ptr().add(count).write(replacement) };
    let updated = unsafe { slice::from_raw_parts_mut(edits.as_mut_ptr(), count + 1) };
    updated.sort_unstable_by_key(|edit| edit.start);
    count + 1
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_range_edit_remove(
    edits: *mut NativeRangeEdit,
    count: usize,
    start: u64,
    end: u64,
) -> usize {
    if edits.is_null() || count == 0 || count > 4096 {
        return usize::MAX;
    }
    let edits = unsafe { slice::from_raw_parts_mut(edits, count) };
    let mut write = 0usize;
    for read in 0..count {
        let edit = edits[read];
        if edit.start == start && edit.end == end {
            continue;
        }
        edits[write] = edit;
        write += 1;
    }
    if write == count {
        usize::MAX
    } else {
        write
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_range_edits_replace(
    edits: *mut NativeRangeEdit,
    count: usize,
    region_length: u64,
) -> bool {
    if count > 4096 || (count > 0 && edits.is_null()) {
        return false;
    }
    if count == 0 {
        return true;
    }
    let edits = unsafe { slice::from_raw_parts_mut(edits, count) };
    for edit in edits.iter_mut() {
        if edit.start >= edit.end
            || edit.end > region_length
            || !edit.gain.is_finite()
            || !(0.0..=16.0).contains(&edit.gain)
        {
            return false;
        }
        let length = edit.end - edit.start;
        edit.fade_in = edit.fade_in.min(length);
        edit.fade_out = edit.fade_out.min(length);
    }
    edits.sort_unstable_by_key(|edit| edit.start);
    true
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCompRange {
    pub start: u64,
    pub end: u64,
    pub fade_in: u64,
    pub fade_out: u64,
}

/// Inputs shared by the realtime Track region scheduler for one bounded block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionBlockConfig {
    pub region_offset: u64,
    pub region_length: u64,
    pub timeline_length: u64,
    pub fade_in: u64,
    pub fade_out: u64,
    pub crossfade_in: u64,
    pub crossfade_out: u64,
    pub clip_gain: f32,
    pub comp_managed: u8,
    pub sample_rate: f64,
    pub source_rate: f64,
    pub source_span: u64,
}

/// Caller-owned block outputs. Fixed storage keeps this scheduler allocation-free.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionBlockOutput {
    pub gain: *mut f32,
    pub pitch_cents: *mut f64,
    pub formant_cents: *mut f64,
    pub matched_note_indices: *mut i64,
    pub previous_note_indices: *mut i64,
    pub source_positions: *mut f64,
    pub local_source_rates: *mut f64,
}

/// Compact control-plane projection used to derive region overlap fades.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionOverlapView {
    pub start: u64,
    pub length: u64,
    pub loop_count: u64,
    pub muted: u8,
    pub crossfade_in: u64,
    pub crossfade_out: u64,
}

fn region_timeline_end(region: &HirariRegionOverlapView) -> u64 {
    if region.loop_count == 0 {
        return u64::MAX;
    }
    let Some(length) = region.length.checked_mul(region.loop_count) else {
        return u64::MAX;
    };
    region.start.saturating_add(length)
}

/// Resolves crossfade spans for immutable region snapshots before publication.
#[no_mangle]
pub unsafe extern "C" fn hirari_regions_apply_overlap_crossfades(
    regions: *mut HirariRegionOverlapView,
    region_count: usize,
) -> bool {
    if region_count != 0 && regions.is_null() {
        return false;
    }
    if region_count == 0 {
        return true;
    }
    let regions = unsafe { slice::from_raw_parts_mut(regions, region_count) };
    for index in 0..regions.len() {
        let region_start = regions[index].start;
        let region_end = region_timeline_end(&regions[index]);
        for other_index in 0..regions.len() {
            if index == other_index {
                continue;
            }
            let other = regions[other_index];
            if other.muted != 0 || other.length == 0 {
                continue;
            }
            let other_end = region_timeline_end(&other);
            if other.start < region_start && other_end > region_start {
                regions[index].crossfade_in = regions[index]
                    .crossfade_in
                    .max(other_end.saturating_sub(region_start).min(8192));
            }
            if other.start > region_start && other.start < region_end {
                regions[index].crossfade_out = regions[index]
                    .crossfade_out
                    .max(region_end.saturating_sub(other.start).min(8192));
            }
        }
    }
    true
}

#[cfg(test)]
mod overlap_tests {
    use super::{hirari_regions_apply_overlap_crossfades, HirariRegionOverlapView};

    #[test]
    fn derives_overlapping_fades_and_preserves_larger_existing_values() {
        let mut regions = [
            HirariRegionOverlapView {
                start: 100,
                length: 100,
                loop_count: 1,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 150,
                length: 80,
                loop_count: 1,
                crossfade_in: 64,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 90,
                length: 500,
                loop_count: 1,
                muted: 1,
                ..Default::default()
            },
        ];
        assert!(unsafe {
            hirari_regions_apply_overlap_crossfades(regions.as_mut_ptr(), regions.len())
        });
        assert_eq!(regions[0].crossfade_out, 50);
        assert_eq!(regions[1].crossfade_in, 64);
        assert_eq!(regions[2].crossfade_in, 0);
    }

    #[test]
    fn caps_long_loop_overlaps_and_saturates_timeline_end_overflow() {
        let mut regions = [
            HirariRegionOverlapView {
                start: 0,
                length: u64::MAX,
                loop_count: 3,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 10,
                length: 2,
                loop_count: 1,
                ..Default::default()
            },
        ];
        assert!(unsafe {
            hirari_regions_apply_overlap_crossfades(regions.as_mut_ptr(), regions.len())
        });
        assert_eq!(regions[1].crossfade_in, 8192);
    }

    #[test]
    fn empty_input_is_valid_and_nonempty_null_input_is_rejected() {
        assert!(unsafe { hirari_regions_apply_overlap_crossfades(std::ptr::null_mut(), 0) });
        assert!(!unsafe { hirari_regions_apply_overlap_crossfades(std::ptr::null_mut(), 1) });
    }
}

/// Computes the non-destructive clip/range/comp gain for one bounded audio
/// block. The caller provides stack- or prepare-time storage so the realtime
/// path does not allocate while moving this work out of Track's C++ renderer.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_gain_block(
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
    if (comp_range_count != 0 && comp_ranges.is_null())
        || (range_edit_count != 0 && range_edits.is_null())
    {
        return;
    }
    let output = slice::from_raw_parts_mut(output, frames as usize);
    let comp_ranges = if comp_range_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(comp_ranges, comp_range_count)
    };
    let range_edits = if range_edit_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(range_edits, range_edit_count)
    };

    for (index, result) in output.iter_mut().enumerate() {
        let relative = region_offset.saturating_add(index as u64);
        if relative >= timeline_length {
            *result = 0.0;
            continue;
        }
        let loop_relative = relative % region_length;
        let mut comp_range = None;
        if comp_managed != 0 {
            let index = comp_ranges.partition_point(|candidate| candidate.start <= loop_relative);
            let Some(candidate) = index
                .checked_sub(1)
                .and_then(|index| comp_ranges.get(index))
            else {
                *result = 0.0;
                continue;
            };
            if loop_relative < candidate.start || loop_relative >= candidate.end {
                *result = 0.0;
                continue;
            }
            comp_range = Some(candidate);
        }

        let mut fade = 1.0f32;
        if fade_in > 0 && relative < fade_in {
            fade = relative as f32 / fade_in as f32;
        }
        if fade_out > 0 && timeline_length > fade_out && relative >= timeline_length - fade_out {
            fade = fade.min((timeline_length - relative) as f32 / fade_out as f32);
        }
        if crossfade_in > 0 && relative < crossfade_in {
            fade = fade.min(relative as f32 / crossfade_in as f32);
        }
        if crossfade_out > 0 && relative >= timeline_length.saturating_sub(crossfade_out) {
            fade = fade.min((timeline_length - relative) as f32 / crossfade_out as f32);
        }

        let mut range_gain = 1.0f32;
        let mut range_fade = 1.0f32;
        let mut range_muted = false;
        for edit in range_edits {
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
                && loop_relative - edit.start < edit.fade_in
            {
                range_fade =
                    range_fade.min((loop_relative - edit.start) as f32 / edit.fade_in as f32);
            }
            if edit.fade_out > 0
                && edit.fade_out <= length
                && loop_relative - edit.start >= length - edit.fade_out
            {
                range_fade =
                    range_fade.min((edit.end - loop_relative) as f32 / edit.fade_out as f32);
            }
            range_muted |= edit.gain == 0.0;
        }
        if range_muted {
            *result = 0.0;
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
                comp_fade = (phase.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin();
            }
            if comp_range.fade_out > 0 && comp_range.end - loop_relative <= comp_range.fade_out {
                let phase = (comp_range.end - loop_relative) as f32 / comp_range.fade_out as f32;
                comp_fade =
                    comp_fade.min((phase.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin());
            }
        }
        *result = clip_gain * (fade * range_fade * range_gain * comp_fade).clamp(0.0, 16.0);
    }
}

/// Prepares all per-frame region metadata consumed by Track's renderer.
/// Gain, note curves, warp mapping, and local source rates now share one Rust
/// scheduler call and caller-owned fixed-size buffers.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_prepare_block(
    config: *const HirariRegionBlockConfig,
    frames: u32,
    comp_ranges: *const NativeCompRange,
    comp_range_count: usize,
    range_edits: *const NativeRangeEdit,
    range_edit_count: usize,
    note_ranges: *const HirariAudioNoteSegmentRange,
    note_range_count: usize,
    note_curves: *const HirariAudioNoteCurveView,
    note_curve_count: usize,
    warp_markers: *const HirariWarpMarker,
    warp_marker_count: usize,
    output: *const HirariRegionBlockOutput,
) -> bool {
    if config.is_null() || output.is_null() {
        return false;
    }
    let config = &*config;
    let output = &*output;
    if frames == 0 {
        return true;
    }
    if config.region_length == 0
        || config.timeline_length == 0
        || !config.clip_gain.is_finite()
        || output.gain.is_null()
        || output.pitch_cents.is_null()
        || output.formant_cents.is_null()
        || output.matched_note_indices.is_null()
        || output.previous_note_indices.is_null()
        || output.source_positions.is_null()
        || output.local_source_rates.is_null()
        || (comp_range_count != 0 && comp_ranges.is_null())
        || (range_edit_count != 0 && range_edits.is_null())
        || (note_range_count != 0
            && (note_ranges.is_null()
                || note_curves.is_null()
                || note_curve_count < note_range_count))
        || (warp_marker_count >= 2 && warp_markers.is_null())
    {
        return false;
    }

    hirari_region_gain_block(
        output.gain,
        frames,
        config.region_offset,
        config.region_length,
        config.timeline_length,
        config.fade_in,
        config.fade_out,
        config.crossfade_in,
        config.crossfade_out,
        config.clip_gain,
        config.comp_managed,
        comp_ranges,
        comp_range_count,
        range_edits,
        range_edit_count,
    );
    hirari_audio_note_curve_block(
        note_ranges,
        note_range_count,
        note_curves,
        note_curve_count,
        config.region_offset,
        config.region_length,
        config.sample_rate,
        frames,
        output.pitch_cents,
        output.formant_cents,
        output.matched_note_indices,
        output.previous_note_indices,
    );
    hirari_region_source_block(
        warp_markers,
        warp_marker_count,
        config.region_offset,
        config.region_length,
        config.source_rate,
        config.source_span,
        frames,
        output.source_positions,
        output.local_source_rates,
    );
    true
}

/// Splits one active Track region into bounded chunks, prepares each chunk's
/// gain/note/warp data, and runs the Rust region renderer. Scratch arrays stay
/// on the audio thread's stack; this path never allocates.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_render_chunks(
    source_left: *const f32,
    source_right: *const f32,
    block_template: *const HirariRegionBlockConfig,
    render_config: *const crate::region_render::HirariRegionRenderConfig,
    comp_ranges: *const NativeCompRange,
    comp_range_count: usize,
    range_edits: *const NativeRangeEdit,
    range_edit_count: usize,
    note_ranges: *const HirariAudioNoteSegmentRange,
    note_range_count: usize,
    note_curves: *const HirariAudioNoteCurveView,
    note_curve_count: usize,
    warp_markers: *const HirariWarpMarker,
    warp_marker_count: usize,
    resample_kernel: *const f32,
    wsola_window: *const f32,
    stretch_handle: *mut std::ffi::c_void,
    destination_left: *mut f32,
    destination_right: *mut f32,
    frame_count: u32,
) {
    if source_left.is_null()
        || source_right.is_null()
        || block_template.is_null()
        || render_config.is_null()
        || destination_left.is_null()
        || destination_right.is_null()
        || resample_kernel.is_null()
        || wsola_window.is_null()
        || frame_count == 0
        || (comp_range_count != 0 && comp_ranges.is_null())
        || (range_edit_count != 0 && range_edits.is_null())
        || (note_range_count != 0 && (note_ranges.is_null() || note_curves.is_null()))
        || (warp_marker_count >= 2 && warp_markers.is_null())
    {
        return;
    }

    let template = unsafe { *block_template };
    let mut render_config = unsafe { *render_config };
    if render_config.spectral_stretch_ready != 0 && stretch_handle.is_null() {
        render_config.spectral_stretch_ready = 0;
    }
    if render_config.spectral_stretch_ready != 0 {
        // Keep stretch controls stable across short slices. Split at loop edges
        // to seek cleanly on each repeated source pass while preserving state
        // between slices within that pass.
        const CONTROL_SLICE: u32 = 64;
        let reference_pitch = if note_range_count > 0 {
            unsafe { (*note_ranges).detected_pitch_cents }
        } else {
            0.0
        };
        let reference_hz = if reference_pitch.is_finite() && reference_pitch > 0.0 {
            440.0 * 2.0f64.powf(((reference_pitch - 6900.0) / 1200.0).clamp(-5.0, 5.0))
        } else {
            140.0
        };
        let lookahead =
            unsafe { crate::region_time_stretch::hirari_region_stretch_lookahead(stretch_handle) }
                as u64;
        let mut cursor = 0u32;
        while cursor < frame_count {
            if template.region_length == 0 {
                render_config.spectral_stretch_ready = 0;
                break;
            }
            let Some(absolute_region_sample) = template.region_offset.checked_add(cursor as u64)
            else {
                render_config.spectral_stretch_ready = 0;
                break;
            };
            let loop_sample = absolute_region_sample % template.region_length;
            let count = CONTROL_SLICE
                .min(frame_count - cursor)
                .min((template.region_length - loop_sample).min(u32::MAX as u64) as u32);
            if count == 0 {
                render_config.spectral_stretch_ready = 0;
                break;
            }
            let source_start = unsafe {
                hirari_region_source_position_at(
                    warp_markers,
                    warp_marker_count,
                    loop_sample,
                    template.source_rate,
                    template.source_span,
                )
            };
            let source_end = unsafe {
                hirari_region_source_position_at(
                    warp_markers,
                    warp_marker_count,
                    loop_sample + count as u64,
                    template.source_rate,
                    template.source_span,
                )
            };
            let parameter_sample = loop_sample
                .saturating_add(lookahead)
                .saturating_add((count / 2) as u64)
                .min(template.region_length - 1);
            let parameter_seconds = parameter_sample as f64 / template.sample_rate.max(1.0);
            let mut segment_indices = [-1i64; 3];
            unsafe {
                crate::audio_note_curve::hirari_audio_note_find_segment_ranges(
                    note_ranges,
                    note_range_count,
                    parameter_seconds,
                    segment_indices.as_mut_ptr(),
                );
            }
            let mut pitch_cents = 0.0;
            let mut formant_cents = 0.0;
            let mut base_hz = reference_hz;
            if segment_indices[2] >= 0 {
                let index = segment_indices[2] as usize;
                if index < note_range_count && index < note_curve_count {
                    let curve = unsafe { &*note_curves.add(index) };
                    let mut values = [0.0f64; 2];
                    unsafe {
                        crate::audio_note_curve::hirari_audio_note_curve_at(
                            curve.anchors,
                            curve.anchor_count,
                            parameter_seconds,
                            curve.pitch_offset_cents,
                            curve.formant_offset_cents,
                            values.as_mut_ptr(),
                        );
                    }
                    pitch_cents = values[0];
                    formant_cents = values[1];
                    let detected_pitch = unsafe { (*note_ranges.add(index)).detected_pitch_cents };
                    if detected_pitch.is_finite() && detected_pitch > 0.0 {
                        base_hz = 440.0
                            * 2.0f64.powf(((detected_pitch - 6900.0) / 1200.0).clamp(-5.0, 5.0));
                    }
                }
            }
            let rendered = unsafe {
                crate::region_time_stretch::hirari_region_stretch_render(
                    stretch_handle,
                    source_left,
                    source_right,
                    render_config.source_samples,
                    render_config.source_offset,
                    render_config.source_span,
                    absolute_region_sample,
                    source_start,
                    source_end,
                    cursor,
                    count,
                    render_config.reverse != 0,
                    (render_config.cache_pitch_semitones as f64 + pitch_cents / 100.0) as f32,
                    (formant_cents / 100.0) as f32,
                    base_hz as f32,
                )
            };
            if !rendered {
                render_config.spectral_stretch_ready = 0;
                break;
            }
            cursor += count;
        }
    }
    let spectral_left = if render_config.spectral_stretch_ready != 0 {
        unsafe { crate::region_time_stretch::hirari_region_stretch_output_left(stretch_handle) }
    } else {
        ptr::null()
    };
    let spectral_right = if render_config.spectral_stretch_ready != 0 {
        unsafe { crate::region_time_stretch::hirari_region_stretch_output_right(stretch_handle) }
    } else {
        ptr::null()
    };
    const CHUNK_FRAMES: u32 = 256;
    let mut chunk_start = 0;
    while chunk_start < frame_count {
        let chunk_frames = CHUNK_FRAMES.min(frame_count - chunk_start);
        let mut config = template;
        let Some(region_offset) = template.region_offset.checked_add(chunk_start as u64) else {
            return;
        };
        config.region_offset = region_offset;

        let mut gain = [0.0f32; CHUNK_FRAMES as usize];
        let mut pitch_cents = [0.0f64; CHUNK_FRAMES as usize];
        let mut formant_cents = [0.0f64; CHUNK_FRAMES as usize];
        let mut matched_note_indices = [-1i64; CHUNK_FRAMES as usize];
        let mut previous_note_indices = [-1i64; CHUNK_FRAMES as usize];
        let mut source_positions = [0.0f64; CHUNK_FRAMES as usize];
        let mut local_source_rates = [0.0f64; CHUNK_FRAMES as usize];
        let block_output = HirariRegionBlockOutput {
            gain: gain.as_mut_ptr(),
            pitch_cents: pitch_cents.as_mut_ptr(),
            formant_cents: formant_cents.as_mut_ptr(),
            matched_note_indices: matched_note_indices.as_mut_ptr(),
            previous_note_indices: previous_note_indices.as_mut_ptr(),
            source_positions: source_positions.as_mut_ptr(),
            local_source_rates: local_source_rates.as_mut_ptr(),
        };
        if unsafe {
            hirari_region_prepare_block(
                &config,
                chunk_frames,
                comp_ranges,
                comp_range_count,
                range_edits,
                range_edit_count,
                note_ranges,
                note_range_count,
                note_curves,
                note_curve_count,
                warp_markers,
                warp_marker_count,
                &block_output,
            )
        } {
            let spectral_left_chunk = if spectral_left.is_null() {
                ptr::null()
            } else {
                unsafe { spectral_left.add(chunk_start as usize) }
            };
            let spectral_right_chunk = if spectral_right.is_null() {
                ptr::null()
            } else {
                unsafe { spectral_right.add(chunk_start as usize) }
            };
            unsafe {
                crate::region_render::hirari_region_render_block(
                    source_left,
                    source_right,
                    &config,
                    &render_config,
                    &block_output,
                    warp_markers,
                    warp_marker_count,
                    note_ranges,
                    note_curve_count,
                    note_curves,
                    note_range_count,
                    resample_kernel,
                    wsola_window,
                    spectral_left_chunk,
                    spectral_right_chunk,
                    destination_left.add(chunk_start as usize),
                    destination_right.add(chunk_start as usize),
                    chunk_frames,
                );
            }
        }
        chunk_start += chunk_frames;
    }
}

#[cfg(test)]
mod chunk_render_tests {
    use super::{hirari_region_render_chunks, HirariRegionBlockConfig, HirariRegionBlockOutput};
    use crate::audio_note_curve::{
        HirariAudioNoteAnchor, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
    };
    use crate::region_render::{hirari_region_render_block, HirariRegionRenderConfig};

    #[test]
    fn chunk_scheduler_matches_manual_chunk_rendering_across_tail_chunk() {
        const SOURCE_FRAMES: usize = 512;
        const OUTPUT_FRAMES: usize = 300;
        let source_left =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.017).sin() * 0.8);
        let source_right =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.023).cos() * 0.6);
        let mut scheduled_left = [0.0; OUTPUT_FRAMES];
        let mut scheduled_right = [0.0; OUTPUT_FRAMES];
        let mut manual_left = [0.0; OUTPUT_FRAMES];
        let mut manual_right = [0.0; OUTPUT_FRAMES];
        let mut window = [0.0f32; 1024];
        window.fill(1.0);
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let block_template = HirariRegionBlockConfig {
            region_length: SOURCE_FRAMES as u64,
            timeline_length: SOURCE_FRAMES as u64,
            clip_gain: 0.75,
            sample_rate: 48_000.0,
            source_rate: 1.0,
            source_span: SOURCE_FRAMES as u64,
            ..Default::default()
        };
        let render_config = HirariRegionRenderConfig {
            source_samples: SOURCE_FRAMES as u64,
            source_span: SOURCE_FRAMES as u64,
            source_rate: 1.0,
            sample_rate: 48_000.0,
            base_pitch_ratio: 1.0,
            minimum_delay: 256.0,
            delay_range: 256.0,
            ..Default::default()
        };

        unsafe {
            hirari_region_render_chunks(
                source_left.as_ptr(),
                source_right.as_ptr(),
                &block_template,
                &render_config,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                kernel,
                window.as_ptr(),
                std::ptr::null_mut(),
                scheduled_left.as_mut_ptr(),
                scheduled_right.as_mut_ptr(),
                OUTPUT_FRAMES as u32,
            );

            let mut chunk_start = 0;
            while chunk_start < OUTPUT_FRAMES {
                let frames = (OUTPUT_FRAMES - chunk_start).min(256);
                let mut config = block_template;
                config.region_offset = chunk_start as u64;
                let mut gain = [0.0f32; 256];
                let mut pitch = [0.0f64; 256];
                let mut formant = [0.0f64; 256];
                let mut matched = [-1i64; 256];
                let mut previous = [-1i64; 256];
                let mut positions = [0.0f64; 256];
                let mut rates = [0.0f64; 256];
                let output = HirariRegionBlockOutput {
                    gain: gain.as_mut_ptr(),
                    pitch_cents: pitch.as_mut_ptr(),
                    formant_cents: formant.as_mut_ptr(),
                    matched_note_indices: matched.as_mut_ptr(),
                    previous_note_indices: previous.as_mut_ptr(),
                    source_positions: positions.as_mut_ptr(),
                    local_source_rates: rates.as_mut_ptr(),
                };
                assert!(super::hirari_region_prepare_block(
                    &config,
                    frames as u32,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    &output,
                ));
                assert!(hirari_region_render_block(
                    source_left.as_ptr(),
                    source_right.as_ptr(),
                    &config,
                    &render_config,
                    &output,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    kernel,
                    window.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    manual_left.as_mut_ptr().add(chunk_start),
                    manual_right.as_mut_ptr().add(chunk_start),
                    frames as u32,
                ));
                chunk_start += frames;
            }
        }

        assert_eq!(scheduled_left, manual_left);
        assert_eq!(scheduled_right, manual_right);
    }

    #[test]
    fn spectral_stretch_scheduler_renders_note_controlled_slices_in_rust() {
        const SOURCE_FRAMES: usize = 8192;
        const OUTPUT_FRAMES: usize = 8192;
        const SAMPLE_RATE: f64 = 48_000.0;
        let source_left =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.031).sin() * 0.7);
        let source_right =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.027).cos() * 0.5);
        let mut destination_left = [0.0f32; OUTPUT_FRAMES];
        let mut destination_right = [0.0f32; OUTPUT_FRAMES];
        let mut window = [1.0f32; 1024];
        window[0] = 0.0;
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let block_template = HirariRegionBlockConfig {
            region_length: SOURCE_FRAMES as u64,
            timeline_length: SOURCE_FRAMES as u64,
            clip_gain: 1.0,
            sample_rate: SAMPLE_RATE,
            source_rate: 1.0,
            source_span: SOURCE_FRAMES as u64,
            ..Default::default()
        };
        let note_range = [HirariAudioNoteSegmentRange {
            start_seconds: 0.0,
            end_seconds: SOURCE_FRAMES as f64 / SAMPLE_RATE,
            detected_pitch_cents: 6900.0,
        }];
        let anchors = [HirariAudioNoteAnchor {
            position_seconds: 0.0,
            pitch_cents: 125.0,
            formant_cents: -40.0,
        }];
        let note_curve = [HirariAudioNoteCurveView {
            anchors: anchors.as_ptr(),
            anchor_count: anchors.len(),
            integral_prefix: std::ptr::null(),
            start_seconds: 0.0,
            end_seconds: SOURCE_FRAMES as f64 / SAMPLE_RATE,
            pitch_offset_cents: 0.0,
            correction_before_seconds: 0.0,
            formant_offset_cents: 0.0,
        }];
        let render_config = HirariRegionRenderConfig {
            source_samples: SOURCE_FRAMES as u64,
            source_span: SOURCE_FRAMES as u64,
            source_rate: 1.0,
            sample_rate: SAMPLE_RATE,
            base_pitch_ratio: 1.0,
            minimum_delay: 256.0,
            delay_range: 256.0,
            cache_pitch_semitones: 0.0,
            spectral_stretch_ready: 1,
            ..Default::default()
        };
        let stretch = crate::region_time_stretch::hirari_region_stretch_create();
        assert!(!stretch.is_null());
        unsafe {
            assert!(crate::region_time_stretch::hirari_region_stretch_prepare(
                stretch,
                SAMPLE_RATE,
                OUTPUT_FRAMES as u32,
            ));
            hirari_region_render_chunks(
                source_left.as_ptr(),
                source_right.as_ptr(),
                &block_template,
                &render_config,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                note_range.as_ptr(),
                note_range.len(),
                note_curve.as_ptr(),
                note_curve.len(),
                std::ptr::null(),
                0,
                kernel,
                window.as_ptr(),
                stretch,
                destination_left.as_mut_ptr(),
                destination_right.as_mut_ptr(),
                OUTPUT_FRAMES as u32,
            );
            assert!(
                !crate::region_time_stretch::hirari_region_stretch_output_left(stretch).is_null()
            );
            crate::region_time_stretch::hirari_region_stretch_destroy(stretch);
        }
        assert!(destination_left.iter().all(|sample| sample.is_finite()));
        assert!(destination_right.iter().all(|sample| sample.is_finite()));
        assert!(destination_left.iter().any(|sample| sample.abs() > 1.0e-6));
        assert!(destination_right.iter().any(|sample| sample.abs() > 1.0e-6));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeType {
    Linear,
    SCurve,
    Exponential,
}

pub struct FadeInfo {
    pub duration_samples: u32,
    pub fade_type: FadeType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionEdit {
    pub id: u64,
    pub label: String,
    pub gain_milli_db: i32,
}
#[derive(Clone, Debug, Default)]
pub struct RegionEditHistory {
    edits: Vec<RegionEdit>,
    redo: Vec<RegionEdit>,
    next_id: u64,
}
impl RegionEditHistory {
    pub fn new() -> Self {
        Self {
            edits: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
        }
    }
    pub fn record(&mut self, label: &str, gain_db: f32) -> Option<u64> {
        if label.trim().is_empty()
            || label.len() > 128
            || label.contains('\0')
            || !gain_db.is_finite()
            || !(-120.0..=24.0).contains(&gain_db)
            || self.edits.len() >= 65_536
        {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.edits.push(RegionEdit {
            id,
            label: label.trim().into(),
            gain_milli_db: (gain_db * 1000.0).round() as i32,
        });
        self.redo.clear();
        Some(id)
    }
    pub fn undo(&mut self) -> Option<RegionEdit> {
        let edit = self.edits.pop()?;
        self.redo.push(edit.clone());
        Some(edit)
    }
    pub fn redo(&mut self) -> Option<RegionEdit> {
        let edit = self.redo.pop()?;
        self.edits.push(edit.clone());
        Some(edit)
    }
    pub fn entries(&self) -> &[RegionEdit] {
        &self.edits
    }
    pub fn redo_entries(&self) -> &[RegionEdit] {
        &self.redo
    }
    pub fn latest_action(&self) -> Option<&str> {
        self.edits.last().map(|edit| edit.label.as_str())
    }
    pub fn clear(&mut self) {
        self.edits.clear();
        self.redo.clear();
    }
    pub fn snapshot(&self) -> Vec<RegionEdit> {
        self.edits.clone()
    }
    pub fn audit(&self) -> bool {
        if self.edits.len() > 65_536 || self.redo.len() > 65_536 || self.next_id == 0 {
            return false;
        }
        let all = self
            .edits
            .iter()
            .chain(self.redo.iter())
            .collect::<Vec<_>>();
        all.iter().all(|e| {
            e.id > 0
                && e.id < self.next_id
                && !e.label.trim().is_empty()
                && e.label.len() <= 128
                && !e.label.contains('\0')
                && (-120_000..=24_000).contains(&e.gain_milli_db)
        }) && all
            .iter()
            .enumerate()
            .all(|(i, e)| all[..i].iter().all(|p| p.id != e.id))
            && self.edits.windows(2).all(|w| w[0].id < w[1].id)
    }
}

pub struct RegionProcessorOrchestrator {
    pub gain: f32,
    pub fade_in: FadeInfo,
    pub fade_out: FadeInfo,
    pub length_samples: u64,
}

impl Default for RegionProcessorOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionProcessorOrchestrator {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            fade_in: FadeInfo {
                duration_samples: 0,
                fade_type: FadeType::Linear,
            },
            fade_out: FadeInfo {
                duration_samples: 0,
                fade_type: FadeType::Linear,
            },
            length_samples: 0,
        }
    }

    /// Applies gain and both edge fades without allocating on the processing path.
    pub fn process_signal(&self, buffer: &mut [f32], offset: usize, size: usize, rel_pos: u64) {
        let end = offset.saturating_add(size).min(buffer.len());
        if offset >= end || !self.gain.is_finite() {
            return;
        }

        let gain = self.gain.clamp(0.0, 4.0);
        for (index, sample) in buffer[offset..end].iter_mut().enumerate() {
            let relative = rel_pos.saturating_add(index as u64);
            let mut fade = 1.0f32;

            if self.fade_in.duration_samples > 0 && relative < self.fade_in.duration_samples as u64
            {
                let t = relative as f32 / self.fade_in.duration_samples as f32;
                fade *= fade_curve(t, self.fade_in.fade_type);
            }

            if self.fade_out.duration_samples > 0
                && self.length_samples > 0
                && relative < self.length_samples
                && relative
                    >= self
                        .length_samples
                        .saturating_sub(self.fade_out.duration_samples as u64)
            {
                let remaining = self.length_samples.saturating_sub(relative);
                let t = (remaining as f32 / self.fade_out.duration_samples as f32).clamp(0.0, 1.0);
                fade *= fade_curve(t, self.fade_out.fade_type);
            }

            let input = if sample.is_finite() { *sample } else { 0.0 };
            let output = input * gain * fade.clamp(0.0, 1.0);
            *sample = if output.is_finite() { output } else { 0.0 };
        }
    }

    /// Applies a reversible region operation while recording its metadata in
    /// the event-processing history. Audio is changed only after validation.
    pub fn process_with_history(
        &self,
        buffer: &mut [f32],
        offset: usize,
        size: usize,
        rel_pos: u64,
        label: &str,
        history: &mut RegionEditHistory,
    ) -> Option<u64> {
        if !self.audit_signal() || label.trim().is_empty() {
            return None;
        }
        let gain_db = 20.0 * self.gain.max(f32::MIN_POSITIVE).log10();
        let id = history.record(label, gain_db)?;
        self.process_signal(buffer, offset, size, rel_pos);
        Some(id)
    }

    pub fn set_gain_db(&mut self, gain_db: f32) -> bool {
        if !gain_db.is_finite() || !(-120.0..=24.0).contains(&gain_db) {
            return false;
        }
        self.gain = 10.0f32.powf(gain_db / 20.0).clamp(0.0, 4.0);
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal synchronization graph.
    pub fn audit_signal(&self) -> bool {
        let valid_fade = |duration: u32| {
            if self.length_samples == 0 {
                duration == 0
            } else {
                (duration as u64) <= self.length_samples
            }
        };
        self.gain.is_finite()
            && (0.0..=4.0).contains(&self.gain)
            && valid_fade(self.fade_in.duration_samples)
            && valid_fade(self.fade_out.duration_samples)
    }
}

fn fade_curve(value: f32, fade_type: FadeType) -> f32 {
    let t = value.clamp(0.0, 1.0);
    match fade_type {
        FadeType::Linear => t,
        FadeType::SCurve => t * t * (3.0 - 2.0 * t),
        FadeType::Exponential => t * t,
    }
}

/// Apply the realtime RegionProcessor gain and fade-in operation to one
/// channel. The C++ owner supplies its already-clamped audio range; all
/// per-sample math and non-finite handling stay in Rust.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_apply_gain_fade(
    samples: *mut f32,
    frames: u32,
    region_relative_pos: u64,
    gain: f32,
    fade_in_samples: u32,
    fade_type: u8,
) -> bool {
    if samples.is_null() || frames == 0 || !gain.is_finite() {
        return false;
    }
    let fade_type = match fade_type {
        0 => FadeType::Linear,
        1 => FadeType::SCurve,
        2 => FadeType::Exponential,
        _ => return false,
    };
    let gain = gain.clamp(0.0, 4.0);
    let samples = unsafe { std::slice::from_raw_parts_mut(samples, frames as usize) };
    for (index, sample) in samples.iter_mut().enumerate() {
        let relative = region_relative_pos.saturating_add(index as u64);
        let envelope = if fade_in_samples > 0 && relative < fade_in_samples as u64 {
            fade_curve(relative as f32 / fade_in_samples as f32, fade_type)
        } else {
            1.0
        };
        let input = if sample.is_finite() { *sample } else { 0.0 };
        let output = input * gain * envelope;
        *sample = if output.is_finite() { output } else { 0.0 };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_range_edits_are_bounded_validated_and_sorted() {
        let mut edits = [
            NativeRangeEdit {
                start: 12,
                end: 40,
                gain: 2.0,
                fade_in: 99,
                fade_out: 2,
            },
            NativeRangeEdit {
                start: 0,
                end: 8,
                gain: 1.0,
                fade_in: 3,
                fade_out: 4,
            },
            NativeRangeEdit {
                start: 7,
                end: 70,
                gain: 0.0,
                fade_in: 50,
                fade_out: 50,
            },
            NativeRangeEdit {
                start: 5,
                end: 5,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            },
            NativeRangeEdit {
                start: 2,
                end: 9,
                gain: f32::NAN,
                fade_in: 0,
                fade_out: 0,
            },
        ];
        let count =
            unsafe { hirari_region_normalize_range_edits(edits.as_mut_ptr(), edits.len(), 20) };
        assert_eq!(count, 3);
        assert_eq!(edits[0].start, 0);
        assert_eq!(edits[0].fade_in, 3);
        assert_eq!(edits[0].fade_out, 4);
        assert_eq!(edits[1].start, 7);
        assert_eq!(edits[1].end, 20);
        assert_eq!(edits[1].fade_in, 13);
        assert_eq!(edits[1].fade_out, 13);
        assert_eq!(edits[2].start, 12);
        assert_eq!(edits[2].end, 20);
        assert_eq!(edits[2].fade_in, 8);
        assert_eq!(edits[2].fade_out, 2);
    }
    use crate::audio_note_curve::{HirariAudioNoteAnchor, HirariAudioNoteCurveView};
    use crate::region_warp::HirariWarpMarker;

    #[test]
    fn realtime_region_gain_fade_sanitizes_and_clamps_samples() {
        let mut samples = [f32::NAN, 1.0, -2.0, 0.5];
        assert!(unsafe {
            hirari_region_processor_apply_gain_fade(
                samples.as_mut_ptr(),
                samples.len() as u32,
                0,
                2.0,
                2,
                0,
            )
        });
        assert_eq!(samples, [0.0, 1.0, -4.0, 1.0]);
        assert!(!unsafe {
            hirari_region_processor_apply_gain_fade(
                samples.as_mut_ptr(),
                samples.len() as u32,
                0,
                1.0,
                2,
                9,
            )
        });
    }

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_region_gain_reference(
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
        );
        fn audio_note_curve_reference_evaluate(
            anchors: *const HirariAudioNoteAnchor,
            count: usize,
            prefix: *const f64,
            start: f64,
            finish: f64,
            pitch_offset: f64,
            formant_offset: f64,
            seconds: f64,
            output: *mut f64,
        );
        fn audio_note_segment_lookup_reference(
            ranges: *const HirariAudioNoteSegmentRange,
            count: usize,
            seconds: f64,
            output: *mut i64,
        );
        fn hirari_region_source_position_reference(
            markers: *const HirariWarpMarker,
            marker_count: usize,
            timeline_sample: u64,
            source_rate: f64,
            source_span: u64,
        ) -> f64;
    }

    #[test]
    #[cfg(feature = "dsp-differential-reference")]
    fn region_gain_block_matches_frozen_cpp_renderer() {
        let comp_ranges = [
            NativeCompRange {
                start: 0,
                end: 32,
                fade_in: 8,
                fade_out: 4,
            },
            NativeCompRange {
                start: 32,
                end: 64,
                fade_in: 4,
                fade_out: 12,
            },
        ];
        let range_edits = [
            NativeRangeEdit {
                start: 0,
                end: 20,
                gain: 0.75,
                fade_in: 6,
                fade_out: 5,
            },
            NativeRangeEdit {
                start: 16,
                end: 44,
                gain: 1.4,
                fade_in: 4,
                fade_out: 8,
            },
            NativeRangeEdit {
                start: 48,
                end: 64,
                gain: 0.0,
                fade_in: 0,
                fade_out: 0,
            },
        ];

        for comp_managed in [0, 1] {
            for offset in [0, 1, 12, 29, 31, 32, 47, 60, 91, 120, 151] {
                let mut rust = [0.0f32; 37];
                let mut cpp = [0.0f32; 37];
                unsafe {
                    hirari_region_gain_block(
                        rust.as_mut_ptr(),
                        rust.len() as u32,
                        offset,
                        64,
                        192,
                        10,
                        14,
                        7,
                        9,
                        1.25,
                        comp_managed,
                        comp_ranges.as_ptr(),
                        comp_ranges.len(),
                        range_edits.as_ptr(),
                        range_edits.len(),
                    );
                    hirari_region_gain_reference(
                        cpp.as_mut_ptr(),
                        cpp.len() as u32,
                        offset,
                        64,
                        192,
                        10,
                        14,
                        7,
                        9,
                        1.25,
                        comp_managed,
                        comp_ranges.as_ptr(),
                        comp_ranges.len(),
                        range_edits.as_ptr(),
                        range_edits.len(),
                    );
                }
                for (index, (rust, cpp)) in rust.iter().zip(cpp).enumerate() {
                    assert!(
                        (rust - cpp).abs() <= 2.0e-6,
                        "gain mismatch at offset={offset}, comp={comp_managed}, frame={index}: Rust={rust}, C++={cpp}"
                    );
                }
            }
        }
    }

    #[test]
    #[cfg(feature = "dsp-differential-reference")]
    fn rust_region_scheduler_block_matches_frozen_cpp_components() {
        let config = HirariRegionBlockConfig {
            region_offset: 5,
            region_length: 8,
            timeline_length: 16,
            fade_in: 3,
            fade_out: 4,
            crossfade_in: 2,
            crossfade_out: 3,
            clip_gain: 1.25,
            comp_managed: 1,
            sample_rate: 1_000.0,
            source_rate: 1.25,
            source_span: 16,
        };
        let comp_ranges = [
            NativeCompRange {
                start: 0,
                end: 4,
                fade_in: 2,
                fade_out: 1,
            },
            NativeCompRange {
                start: 4,
                end: 8,
                fade_in: 1,
                fade_out: 2,
            },
        ];
        let range_edits = [NativeRangeEdit {
            start: 1,
            end: 7,
            gain: 0.75,
            fade_in: 2,
            fade_out: 3,
        }];
        let note_ranges = [
            HirariAudioNoteSegmentRange {
                start_seconds: 0.0,
                end_seconds: 0.004,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 0.002,
                end_seconds: 0.007,

                detected_pitch_cents: 0.0,
            },
        ];
        let anchors = [
            [
                HirariAudioNoteAnchor {
                    position_seconds: 0.0,
                    pitch_cents: -300.0,
                    formant_cents: 125.0,
                },
                HirariAudioNoteAnchor {
                    position_seconds: 0.004,
                    pitch_cents: 450.0,
                    formant_cents: -240.0,
                },
            ],
            [
                HirariAudioNoteAnchor {
                    position_seconds: 0.002,
                    pitch_cents: 175.0,
                    formant_cents: -75.0,
                },
                HirariAudioNoteAnchor {
                    position_seconds: 0.007,
                    pitch_cents: -625.0,
                    formant_cents: 350.0,
                },
            ],
        ];
        let pitch_offsets = [85.0, -40.0];
        let formant_offsets = [-30.0, 65.0];
        let mut prefixes = [[0.0; 2]; 2];
        let mut curves = Vec::new();
        for index in 0..2 {
            unsafe {
                crate::audio_note_curve::hirari_audio_note_curve_build_integral_prefix(
                    anchors[index].as_ptr(),
                    anchors[index].len(),
                    pitch_offsets[index],
                    prefixes[index].as_mut_ptr(),
                );
            }
            curves.push(HirariAudioNoteCurveView {
                anchors: anchors[index].as_ptr(),
                anchor_count: anchors[index].len(),
                integral_prefix: prefixes[index].as_ptr(),
                start_seconds: note_ranges[index].start_seconds,
                end_seconds: note_ranges[index].end_seconds,
                pitch_offset_cents: pitch_offsets[index],
                correction_before_seconds: 0.0,
                formant_offset_cents: formant_offsets[index],
            });
        }
        let warp_markers = [
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 12,
                timeline_sample: 8,
                transient: 1,
            },
        ];

        const FRAMES: usize = 13;
        let mut output_gain = [0.0f32; FRAMES];
        let mut output_pitch = [0.0f64; FRAMES];
        let mut output_formant = [0.0f64; FRAMES];
        let mut output_matched = [-1i64; FRAMES];
        let mut output_previous = [-1i64; FRAMES];
        let mut output_positions = [0.0f64; FRAMES];
        let mut output_rates = [0.0f64; FRAMES];
        let output = HirariRegionBlockOutput {
            gain: output_gain.as_mut_ptr(),
            pitch_cents: output_pitch.as_mut_ptr(),
            formant_cents: output_formant.as_mut_ptr(),
            matched_note_indices: output_matched.as_mut_ptr(),
            previous_note_indices: output_previous.as_mut_ptr(),
            source_positions: output_positions.as_mut_ptr(),
            local_source_rates: output_rates.as_mut_ptr(),
        };
        assert_eq!(std::mem::size_of::<HirariRegionBlockConfig>(), 88);
        assert_eq!(std::mem::size_of::<HirariRegionBlockOutput>(), 56);
        assert!(unsafe {
            hirari_region_prepare_block(
                &config,
                FRAMES as u32,
                comp_ranges.as_ptr(),
                comp_ranges.len(),
                range_edits.as_ptr(),
                range_edits.len(),
                note_ranges.as_ptr(),
                note_ranges.len(),
                curves.as_ptr(),
                curves.len(),
                warp_markers.as_ptr(),
                warp_markers.len(),
                &output,
            )
        });

        let mut cpp_gain = [0.0f32; FRAMES];
        unsafe {
            hirari_region_gain_reference(
                cpp_gain.as_mut_ptr(),
                FRAMES as u32,
                config.region_offset,
                config.region_length,
                config.timeline_length,
                config.fade_in,
                config.fade_out,
                config.crossfade_in,
                config.crossfade_out,
                config.clip_gain,
                config.comp_managed,
                comp_ranges.as_ptr(),
                comp_ranges.len(),
                range_edits.as_ptr(),
                range_edits.len(),
            );
        }
        for frame in 0..FRAMES {
            assert!((output_gain[frame] - cpp_gain[frame]).abs() <= 1.0e-6);
            let relative = config.region_offset + frame as u64;
            let loop_sample = relative % config.region_length;
            let seconds = loop_sample as f64 / config.sample_rate;
            let mut expected_indices = [-1i64; 3];
            unsafe {
                audio_note_segment_lookup_reference(
                    note_ranges.as_ptr(),
                    note_ranges.len(),
                    seconds,
                    expected_indices.as_mut_ptr(),
                );
            }
            assert_eq!(output_matched[frame], expected_indices[0]);
            assert_eq!(output_previous[frame], expected_indices[1]);
            if expected_indices[0] >= 0 {
                let curve = &curves[expected_indices[0] as usize];
                let mut expected_curve = [0.0f64; 3];
                unsafe {
                    audio_note_curve_reference_evaluate(
                        curve.anchors,
                        curve.anchor_count,
                        curve.integral_prefix,
                        curve.start_seconds,
                        curve.end_seconds,
                        curve.pitch_offset_cents,
                        curve.formant_offset_cents,
                        seconds,
                        expected_curve.as_mut_ptr(),
                    );
                }
                assert!((output_pitch[frame] - expected_curve[0]).abs() <= 1.0e-10);
                assert!((output_formant[frame] - expected_curve[1]).abs() <= 1.0e-10);
            } else {
                assert_eq!((output_pitch[frame], output_formant[frame]), (0.0, 0.0));
            }

            let expected_position = unsafe {
                hirari_region_source_position_reference(
                    warp_markers.as_ptr(),
                    warp_markers.len(),
                    loop_sample,
                    config.source_rate,
                    config.source_span,
                )
            };
            let expected_rate = if loop_sample + 1 < config.region_length {
                unsafe {
                    hirari_region_source_position_reference(
                        warp_markers.as_ptr(),
                        warp_markers.len(),
                        loop_sample + 1,
                        config.source_rate,
                        config.source_span,
                    ) - expected_position
                }
            } else if loop_sample > 0 {
                expected_position
                    - unsafe {
                        hirari_region_source_position_reference(
                            warp_markers.as_ptr(),
                            warp_markers.len(),
                            loop_sample - 1,
                            config.source_rate,
                            config.source_span,
                        )
                    }
            } else {
                1.0
            };
            assert!((output_positions[frame] - expected_position).abs() <= 1.0e-12);
            assert!((output_rates[frame] - expected_rate).abs() <= 1.0e-12);
        }
    }

    #[test]
    fn applies_gain_and_edge_fades() {
        let processor = RegionProcessorOrchestrator {
            gain: 2.0,
            fade_in: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            fade_out: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            length_samples: 4,
        };
        let mut buffer = [1.0; 4];
        processor.process_signal(&mut buffer, 0, 4, 0);
        assert_eq!(buffer, [0.0, 1.0, 2.0, 1.0]);
    }

    #[test]
    fn clamps_invalid_range_without_panicking() {
        let processor = RegionProcessorOrchestrator::new();
        let mut buffer = [f32::NAN, 1.0, 2.0];
        processor.process_signal(&mut buffer, 2, 100, 0);
        assert!(buffer[0].is_nan());
        assert_eq!(&buffer[1..], &[1.0, 2.0]);
    }

    #[test]
    fn processing_records_event_history_after_validation() {
        let mut processor = RegionProcessorOrchestrator::new();
        assert!(processor.set_gain_db(6.0));
        let mut history = RegionEditHistory::new();
        let mut buffer = [1.0; 2];
        let id = processor
            .process_with_history(&mut buffer, 0, 2, 0, "clip gain", &mut history)
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(
            history.entries().last().map(|entry| entry.label.as_str()),
            Some("clip gain")
        );
        assert!(buffer[0] > 1.0);
    }
}
