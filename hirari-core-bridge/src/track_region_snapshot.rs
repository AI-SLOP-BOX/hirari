//! Control-thread publication of a track's region render inputs.
//!
//! C++ owns the decoded audio buffers and the `Region` copies that keep them
//! alive for a published snapshot. Everything derived from those copies — the
//! overlap crossfades, the spectral-stretch decision and the field-by-field
//! render input the audio callback reads — is assembled here so the C++ side
//! only has to hand over the raw pointers and the editable state.

use std::ffi::c_void;
use std::ptr;
use std::slice;

use crate::audio_note_curve::{HirariAudioNoteCurveView, HirariAudioNoteSegmentRange};
use crate::region_clip_edits::RegionClipEditsView;
use crate::region_edit_host::HirariRegionState;
use crate::region_processing::{
    hirari_regions_apply_overlap_crossfades, HirariRegionOverlapView,
};
use crate::region_render::hirari_region_needs_spectral_stretch;
use crate::track_region_render::HirariTrackRegionRenderInput;

/// Matches the engine's practical region limit so a corrupt count cannot make
/// the audio-thread snapshot allocate without bound.
pub const HIRARI_REGION_SNAPSHOT_MAX: usize = 1 << 20;

/// One region as the control thread sees it when it publishes a snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HirariRegionRenderSource {
    pub state: HirariRegionState,
    pub id: u32,
    pub muted: u8,
    pub audio_left: *const f32,
    pub audio_right: *const f32,
    pub audio_samples: u64,
    pub clip: *const RegionClipEditsView,
    pub note_ranges: *const HirariAudioNoteSegmentRange,
    pub note_range_count: usize,
    pub note_curves: *const HirariAudioNoteCurveView,
    pub note_curve_count: usize,
    pub note_cache_segments: usize,
    pub stretch_handle: *mut c_void,
    pub crossfade_in: u64,
    pub crossfade_out: u64,
}

impl Default for HirariRegionRenderSource {
    fn default() -> Self {
        Self {
            state: HirariRegionState::default(),
            id: 0,
            muted: 0,
            audio_left: ptr::null(),
            audio_right: ptr::null(),
            audio_samples: 0,
            clip: ptr::null(),
            note_ranges: ptr::null(),
            note_range_count: 0,
            note_curves: ptr::null(),
            note_curve_count: 0,
            note_cache_segments: 0,
            stretch_handle: ptr::null_mut(),
            crossfade_in: 0,
            crossfade_out: 0,
        }
    }
}

/// Builds `count` render inputs. Returns the number written, or 0 when the
/// arguments are unusable or the overlap pass rejects the set.
///
/// # Safety
/// `sources` and `out` must each be valid for `count` elements, and every
/// pointer inside a source must stay valid for as long as the snapshot lives.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_build_region_render_inputs(
    sources: *const HirariRegionRenderSource,
    count: usize,
    sample_rate: f64,
    resample_kernel: *const f32,
    wsola_window: *const f32,
    cache_owner: usize,
    cache_snapshot: usize,
    out: *mut HirariTrackRegionRenderInput,
    capacity: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    if sources.is_null() || out.is_null() || count > capacity || count > HIRARI_REGION_SNAPSHOT_MAX
    {
        return 0;
    }
    let sources = unsafe { slice::from_raw_parts(sources, count) };
    let out = unsafe { slice::from_raw_parts_mut(out, count) };

    let mut overlap: Vec<HirariRegionOverlapView> = sources
        .iter()
        .map(|source| HirariRegionOverlapView {
            start: source.state.start,
            length: source.state.len,
            loop_count: source.state.loop_count as u64,
            muted: source.muted,
            crossfade_in: source.crossfade_in,
            crossfade_out: source.crossfade_out,
        })
        .collect();
    if !unsafe { hirari_regions_apply_overlap_crossfades(overlap.as_mut_ptr(), overlap.len()) } {
        return 0;
    }

    for (index, source) in sources.iter().enumerate() {
        let state = source.state;
        let clip = if source.clip.is_null() {
            None
        } else {
            Some(unsafe { &*source.clip })
        };
        let (range_edits, range_edit_count, comp_ranges, comp_range_count, warp_markers,
            warp_marker_count, comp_managed) = match clip {
            Some(clip) => (
                clip.range_edits,
                clip.range_edit_count,
                clip.comp_ranges.cast(),
                clip.comp_range_count,
                clip.warp_markers,
                clip.warp_marker_count,
                clip.comp_managed,
            ),
            None => (ptr::null(), 0, ptr::null(), 0, ptr::null(), 0, 0),
        };
        let needs_spectral_stretch = unsafe {
            hirari_region_needs_spectral_stretch(
                state.source_sample_rate,
                state.timeline_sample_rate,
                state.warp_ratio,
                warp_marker_count,
                state.pitch_semitones,
                source.note_curves,
                source.note_curve_count,
            )
        };
        out[index] = HirariTrackRegionRenderInput {
            source_left: source.audio_left,
            source_right: source.audio_right,
            destination_left: ptr::null_mut(),
            destination_right: ptr::null_mut(),
            comp_ranges,
            comp_range_count,
            range_edits,
            range_edit_count,
            note_ranges: source.note_ranges,
            note_range_count: source.note_range_count,
            note_curves: source.note_curves,
            note_curve_count: source.note_curve_count,
            warp_markers,
            warp_marker_count,
            resample_kernel,
            wsola_window,
            stretch_handle: source.stretch_handle,
            cache_owner,
            cache_snapshot,
            cache_note_segments: source.note_cache_segments,
            playhead: 0,
            region_start: state.start,
            region_length: state.len,
            loop_count: state.loop_count as u64,
            source_samples: source.audio_samples,
            source_offset: state.source_offset,
            source_length: state.source_length,
            fade_in: state.fade_in_samples,
            fade_out: state.fade_out_samples,
            crossfade_in: overlap[index].crossfade_in,
            crossfade_out: overlap[index].crossfade_out,
            warp_ratio: state.warp_ratio,
            source_sample_rate: state.source_sample_rate,
            timeline_sample_rate: state.timeline_sample_rate,
            sample_rate,
            clip_gain: state.clip_gain,
            pitch_semitones: state.pitch_semitones,
            frame_count: 0,
            region_id: source.id,
            sync_group: state.sync_group,
            muted: if source.muted != 0 { 1 } else { 0 },
            comp_managed,
            reverse: state.reverse as u8,
            pitch_preserve_warp: state.pitch_preserve_warp as u8,
            needs_spectral_stretch: needs_spectral_stretch as u8,
        };
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(start: u64, len: u64, sample_rate: f64) -> HirariRegionState {
        HirariRegionState {
            start,
            len,
            loop_count: 1,
            warp_ratio: 1.0,
            clip_gain: 1.0,
            source_sample_rate: sample_rate,
            timeline_sample_rate: sample_rate,
            ..Default::default()
        }
    }

    #[test]
    fn crosses_overlapping_regions_and_maps_state_fields() {
        let sources = [
            HirariRegionRenderSource {
                state: state(0, 1000, 48_000.0),
                id: 7,
                ..Default::default()
            },
            HirariRegionRenderSource {
                state: state(500, 1000, 48_000.0),
                id: 9,
                ..Default::default()
            },
        ];
        let mut out = [HirariTrackRegionRenderInput::default(); 2];
        let written = unsafe {
            hirari_track_build_region_render_inputs(
                sources.as_ptr(),
                sources.len(),
                48_000.0,
                ptr::null(),
                ptr::null(),
                1,
                2,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        assert_eq!(written, 2);
        assert_eq!(out[0].region_id, 7);
        assert_eq!(out[1].region_start, 500);
        assert_eq!(out[0].crossfade_out, 500);
        assert_eq!(out[1].crossfade_in, 500);
        assert_eq!(out[0].cache_owner, 1);
        assert_eq!(out[0].cache_snapshot, 2);
        assert_eq!(out[0].sample_rate, 48_000.0);
        assert_eq!(out[0].needs_spectral_stretch, 0);
    }

    #[test]
    fn rejects_unusable_arguments() {
        let source = HirariRegionRenderSource::default();
        let mut out = [HirariTrackRegionRenderInput::default(); 1];
        unsafe {
            assert_eq!(
                hirari_track_build_region_render_inputs(
                    ptr::null(),
                    1,
                    48_000.0,
                    ptr::null(),
                    ptr::null(),
                    0,
                    0,
                    out.as_mut_ptr(),
                    1,
                ),
                0
            );
            assert_eq!(
                hirari_track_build_region_render_inputs(
                    &source,
                    2,
                    48_000.0,
                    ptr::null(),
                    ptr::null(),
                    0,
                    0,
                    out.as_mut_ptr(),
                    1,
                ),
                0
            );
            assert_eq!(
                hirari_track_build_region_render_inputs(
                    &source,
                    1,
                    48_000.0,
                    ptr::null(),
                    ptr::null(),
                    0,
                    0,
                    out.as_mut_ptr(),
                    1,
                ),
                1
            );
        }
    }
}
