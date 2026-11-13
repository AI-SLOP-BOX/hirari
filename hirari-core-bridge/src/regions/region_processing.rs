use crate::audio_note_curve::{
    hirari_audio_note_curve_block, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
};
use crate::region_edit_host::HirariRegionState;
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

/// Applies the scalar invariants for a newly inserted region. Audio ownership
/// stays in C++; all shared range and overflow policy lives in the Rust core.
/// The caller's editable state is updated in place, so the C++ side applies
/// one state record instead of shuttling a second metadata copy.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_normalize_add_metadata(
    state: *mut HirariRegionState,
    has_audio: bool,
    audio_samples: u64,
    audio_channels: u32,
) -> bool {
    let Some(state) = state.as_mut() else {
        return false;
    };
    if state.len == 0 || state.start.checked_add(state.len).is_none() {
        return false;
    }
    if !state.warp_ratio.is_finite() || !(0.5..=2.0).contains(&state.warp_ratio) {
        state.warp_ratio = 1.0;
    }
    if !state.pitch_semitones.is_finite() || !(-24.0..=24.0).contains(&state.pitch_semitones) {
        state.pitch_semitones = 0.0;
    }
    if state.loop_count == 0
        || state.loop_count > 1024
        || state.len > u64::MAX / u64::from(state.loop_count)
    {
        state.loop_count = 1;
    }
    let Some(timeline_length) = state.len.checked_mul(u64::from(state.loop_count)) else {
        return false;
    };
    if state.start.checked_add(timeline_length).is_none() {
        return false;
    }
    if !state.clip_gain.is_finite() {
        state.clip_gain = 1.0;
    }
    state.clip_gain = state.clip_gain.clamp(0.0, 2.0);

    if has_audio {
        if audio_channels == 0 || audio_samples == 0 || state.source_offset >= audio_samples {
            return false;
        }
        let available = audio_samples - state.source_offset;
        state.source_length = if state.source_length == 0 {
            available
        } else {
            state.source_length.min(available)
        };
        if state.source_length == 0 {
            return false;
        }
        if state.base_length == 0 {
            state.base_length = state.source_length;
        }
        if state.base_start == 0 {
            state.base_start = state.start;
        }
        if state.base_source_offset == 0 {
            state.base_source_offset = state.source_offset;
        }
    } else if state.source_length == 0 {
        state.source_length = state.len;
    }

    // Fades are bounded by the playable length, as the loader used to clamp
    // them next to the insertion.
    state.fade_in_samples = state.fade_in_samples.min(state.len);
    state.fade_out_samples = state.fade_out_samples.min(state.len);

    state.start.checked_add(state.len).is_some()
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

/// Validates, clips, and orders a Track region's non-destructive range edits.
/// The realtime renderer consumes this normalized array without extra scans
/// for malformed project data.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCompRange {
    pub start: u64,
    pub end: u64,
    pub fade_in: u64,
    pub fade_out: u64,
}

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
// ---------------------------------------------------------------------------
// Scalar region edit policy
// ---------------------------------------------------------------------------
//
// Clip gain, mute, reverse, pitch, loop count, sync group and fades each had
// their numeric bounds duplicated in two C++ places: the `Track` storage
// mutators and the `HirariUnifiedEngine` wrappers that build undo snapshots.
// When the two disagreed, an edit could be accepted at the UI/undo layer but
// silently dropped (or clamped differently) by the storage layer.
//
// The policy now lives here, in Rust, and both C++ layers call it. `Track` still
// owns the storage and the immutable-snapshot publication; the bounds no longer
// exist in C++ at all.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HirariRegionScalarField {
    #[default]
    ClipGain = 1,
    Muted = 2,
    Reverse = 3,
    PitchSemitones = 4,
    LoopCount = 5,
    SyncGroup = 6,
    Fades = 7,
    Locked = 8,
    PitchPreserveWarp = 9,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionScalarEdit {
    pub field: HirariRegionScalarField,
    pub locked: bool,
    /// Primary numeric argument: gain / semitones / loop count, or fade-in.
    pub value_a: f32,
    /// Secondary numeric argument: fade-out only.
    pub value_b: f32,
    /// Region geometry, needed only by the LoopCount overflow guard.
    pub region_len: u64,
    pub region_start: u64,
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

#[path = "region_processing_scalar.rs"]
mod region_processing_scalar;
pub use region_processing_scalar::*;
#[path = "region_processing_orchestra.rs"]
mod region_processing_orchestra;
pub use region_processing_orchestra::*;
