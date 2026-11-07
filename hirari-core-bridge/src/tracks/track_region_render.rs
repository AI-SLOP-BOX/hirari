use crate::audio_note_curve::{HirariAudioNoteCurveView, HirariAudioNoteSegmentRange};
use crate::region_pitch_correction::hirari_region_pitch_correction_delays;
use crate::region_processing::{
    hirari_region_render_chunks, HirariRegionBlockConfig, NativeCompRange, NativeRangeEdit,
};
use crate::region_render::HirariRegionRenderConfig;
use crate::region_warp::HirariWarpMarker;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariTrackRegionRenderInput {
    pub source_left: *const f32,
    pub source_right: *const f32,
    pub destination_left: *mut f32,
    pub destination_right: *mut f32,
    pub comp_ranges: *const NativeCompRange,
    pub comp_range_count: usize,
    pub range_edits: *const NativeRangeEdit,
    pub range_edit_count: usize,
    pub note_ranges: *const HirariAudioNoteSegmentRange,
    pub note_range_count: usize,
    pub note_curves: *const HirariAudioNoteCurveView,
    pub note_curve_count: usize,
    pub warp_markers: *const HirariWarpMarker,
    pub warp_marker_count: usize,
    pub resample_kernel: *const f32,
    pub wsola_window: *const f32,
    pub stretch_handle: *mut std::ffi::c_void,
    pub cache_owner: usize,
    pub cache_snapshot: usize,
    pub cache_note_segments: usize,
    pub playhead: u64,
    pub region_start: u64,
    pub region_length: u64,
    pub loop_count: u64,
    pub source_samples: u64,
    pub source_offset: u64,
    pub source_length: u64,
    pub fade_in: u64,
    pub fade_out: u64,
    pub crossfade_in: u64,
    pub crossfade_out: u64,
    pub warp_ratio: f64,
    pub source_sample_rate: f64,
    pub timeline_sample_rate: f64,
    pub sample_rate: f64,
    pub clip_gain: f32,
    pub pitch_semitones: f32,
    pub frame_count: u32,
    pub region_id: u32,
    pub sync_group: u32,
    pub muted: u8,
    pub comp_managed: u8,
    pub reverse: u8,
    pub pitch_preserve_warp: u8,
    pub needs_spectral_stretch: u8,
}

/// Resolves an active region's timeline intersection, derives its DSP configs,
/// and renders it without exposing C++ Region or AudioBuffer objects to the
/// realtime policy. All storage remains owned by the immutable Track snapshot.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_render_region(
    input: *const HirariTrackRegionRenderInput,
) -> bool {
    if input.is_null() {
        return false;
    }
    let input = unsafe { &*input };
    if input.muted != 0
        || input.frame_count == 0
        || input.region_length == 0
        || input.loop_count == 0
        || input.destination_left.is_null()
        || input.destination_right.is_null()
        || input.source_left.is_null()
        || input.source_right.is_null()
        || input.resample_kernel.is_null()
        || input.wsola_window.is_null()
        || (input.comp_range_count != 0 && input.comp_ranges.is_null())
        || (input.range_edit_count != 0 && input.range_edits.is_null())
        || (input.note_range_count != 0
            && (input.note_ranges.is_null()
                || input.note_curves.is_null()
                || input.note_curve_count < input.note_range_count))
        || (input.warp_marker_count >= 2 && input.warp_markers.is_null())
    {
        return false;
    }
    let Some(timeline_length) = input.region_length.checked_mul(input.loop_count) else {
        return false;
    };
    if (input.playhead >= input.region_start
        && input.playhead - input.region_start >= timeline_length)
        || (input.playhead < input.region_start
            && input.region_start - input.playhead >= input.frame_count as u64)
    {
        return false;
    }

    let first = input.playhead.max(input.region_start);
    let region_offset = first - input.region_start;
    let destination_offset = first - input.playhead;
    if destination_offset >= input.frame_count as u64 {
        return false;
    }
    let available = (input.frame_count as u64 - destination_offset)
        .min(timeline_length - region_offset)
        .min(u32::MAX as u64) as u32;
    if available == 0 {
        return false;
    }

    let source_rate = if input.source_sample_rate.is_finite() && input.source_sample_rate > 0.0 {
        input.source_sample_rate
    } else {
        input.timeline_sample_rate
    };
    let timeline_rate =
        if input.timeline_sample_rate.is_finite() && input.timeline_sample_rate > 0.0 {
            input.timeline_sample_rate
        } else {
            source_rate
        };
    let converted_rate = source_rate / timeline_rate;
    let effective_source_rate = input.warp_ratio * converted_rate;
    let source_rate = if effective_source_rate.is_finite() && effective_source_rate > 0.0 {
        effective_source_rate
    } else {
        input.warp_ratio
    };
    let base_pitch_ratio = 2.0f64.powf(input.pitch_semitones as f64 / 12.0);
    if !base_pitch_ratio.is_finite() || base_pitch_ratio <= 0.0 {
        return false;
    }
    let source_span = if input.source_length > 0 {
        input.source_length
    } else {
        input.region_length
    };
    let reference_pitch = if input.note_range_count == 0 {
        0.0
    } else {
        unsafe { (*input.note_ranges).detected_pitch_cents }
    };
    let mut pitch_delays = [0.0f64; 3];
    if !unsafe {
        hirari_region_pitch_correction_delays(
            input.sample_rate,
            reference_pitch,
            pitch_delays.as_mut_ptr(),
        )
    } {
        return false;
    }

    let block = HirariRegionBlockConfig {
        region_offset,
        region_length: input.region_length,
        timeline_length,
        fade_in: input.fade_in,
        fade_out: input.fade_out,
        crossfade_in: input.crossfade_in,
        crossfade_out: input.crossfade_out,
        clip_gain: input.clip_gain,
        comp_managed: input.comp_managed,
        sample_rate: input.sample_rate,
        source_rate,
        source_span,
    };
    let render = HirariRegionRenderConfig {
        source_samples: input.source_samples,
        source_offset: input.source_offset,
        source_span,
        source_rate,
        sample_rate: input.sample_rate,
        base_pitch_ratio,
        minimum_delay: pitch_delays[0],
        delay_range: pitch_delays[1],
        cache_owner: input.cache_owner,
        cache_snapshot: input.cache_snapshot,
        cache_source: input.source_left as usize,
        cache_region_id: input.region_id,
        cache_note_segments: input.cache_note_segments,
        cache_note_segment_count: input.note_range_count,
        cache_pitch_semitones: input.pitch_semitones,
        sync_group: input.sync_group,
        reverse: input.reverse,
        pitch_preserve_warp: input.pitch_preserve_warp,
        spectral_stretch_ready: u8::from(
            input.pitch_preserve_warp != 0
                && input.needs_spectral_stretch != 0
                && !input.stretch_handle.is_null(),
        ),
    };
    unsafe {
        hirari_region_render_chunks(
            input.source_left,
            input.source_right,
            &block,
            &render,
            input.comp_ranges,
            input.comp_range_count,
            input.range_edits,
            input.range_edit_count,
            input.note_ranges,
            input.note_range_count,
            input.note_curves,
            input.note_curve_count,
            input.warp_markers,
            input.warp_marker_count,
            input.resample_kernel,
            input.wsola_window,
            input.stretch_handle,
            input.destination_left.add(destination_offset as usize),
            input.destination_right.add(destination_offset as usize),
            available,
        );
    }
    true
}

/// Renders every immutable region descriptor for one track block. The
/// descriptor array is prepared and retained by the control thread, so this
/// realtime traversal needs no allocation and leaves C++ only as the audio
/// buffer/snapshot ABI adapter.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_render_regions(
    inputs: *const HirariTrackRegionRenderInput,
    input_count: usize,
    playhead: u64,
    frame_count: u32,
    destination_left: *mut f32,
    destination_right: *mut f32,
    phase_invert: u8,
) -> usize {
    if (input_count != 0 && inputs.is_null())
        || frame_count == 0
        || destination_left.is_null()
        || destination_right.is_null()
    {
        return 0;
    }
    let mut rendered = 0;
    if input_count != 0 {
        let inputs = unsafe { std::slice::from_raw_parts(inputs, input_count) };
        for descriptor in inputs {
            let mut input = *descriptor;
            input.playhead = playhead;
            input.frame_count = frame_count;
            input.destination_left = destination_left;
            input.destination_right = destination_right;
            if unsafe { hirari_track_render_region(&input) } {
                rendered += 1;
            }
        }
    }
    if phase_invert != 0 {
        let left =
            unsafe { std::slice::from_raw_parts_mut(destination_left, frame_count as usize) };
        let right =
            unsafe { std::slice::from_raw_parts_mut(destination_right, frame_count as usize) };
        for sample in left.iter_mut().chain(right.iter_mut()) {
            *sample = -*sample;
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_track_render_region, hirari_track_render_regions, HirariTrackRegionRenderInput,
    };

    #[test]
    fn rust_region_orchestrator_bounds_and_renders_active_audio() {
        assert_eq!(std::mem::size_of::<HirariTrackRegionRenderInput>(), 312);
        assert_eq!(
            std::mem::offset_of!(HirariTrackRegionRenderInput, playhead),
            160
        );
        assert_eq!(
            std::mem::offset_of!(HirariTrackRegionRenderInput, warp_ratio),
            248
        );
        assert_eq!(
            std::mem::offset_of!(HirariTrackRegionRenderInput, muted),
            300
        );
        const FRAMES: usize = 512;
        let source_left = std::array::from_fn::<_, FRAMES, _>(|i| (i as f32 * 0.03).sin());
        let source_right = std::array::from_fn::<_, FRAMES, _>(|i| (i as f32 * 0.027).cos());
        let mut destination_left = [0.0f32; FRAMES];
        let mut destination_right = [0.0f32; FRAMES];
        let mut window = [1.0f32; 1024];
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let mut input = HirariTrackRegionRenderInput {
            source_left: source_left.as_ptr(),
            source_right: source_right.as_ptr(),
            destination_left: destination_left.as_mut_ptr(),
            destination_right: destination_right.as_mut_ptr(),
            resample_kernel: kernel,
            wsola_window: window.as_mut_ptr(),
            region_start: 32,
            region_length: FRAMES as u64,
            loop_count: 1,
            source_samples: FRAMES as u64,
            source_length: FRAMES as u64,
            warp_ratio: 1.0,
            sample_rate: 48_000.0,
            clip_gain: 1.0,
            frame_count: FRAMES as u32,
            ..Default::default()
        };
        assert!(unsafe { hirari_track_render_region(&input) });
        assert!(destination_left[..32].iter().all(|sample| *sample == 0.0));
        assert!(destination_right[..32].iter().all(|sample| *sample == 0.0));
        assert!(destination_left.iter().any(|sample| sample.abs() > 1.0e-6));
        assert!(destination_right.iter().any(|sample| sample.abs() > 1.0e-6));

        destination_left.fill(0.0);
        destination_right.fill(0.0);
        input.playhead = FRAMES as u64 + 32;
        assert!(!unsafe { hirari_track_render_region(&input) });
        assert!(destination_left.iter().all(|sample| *sample == 0.0));
        assert!(destination_right.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn rust_track_scheduler_walks_immutable_region_snapshot() {
        const FRAMES: usize = 512;
        let source_left = std::array::from_fn::<_, FRAMES, _>(|i| (i as f32 * 0.03).sin());
        let source_right = std::array::from_fn::<_, FRAMES, _>(|i| (i as f32 * 0.027).cos());
        let mut destination_left = [0.0f32; FRAMES];
        let mut destination_right = [0.0f32; FRAMES];
        let mut window = [1.0f32; 1024];
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let base = HirariTrackRegionRenderInput {
            source_left: source_left.as_ptr(),
            source_right: source_right.as_ptr(),
            resample_kernel: kernel,
            wsola_window: window.as_mut_ptr(),
            region_start: 32,
            region_length: FRAMES as u64,
            loop_count: 1,
            source_samples: FRAMES as u64,
            source_length: FRAMES as u64,
            warp_ratio: 1.0,
            sample_rate: 48_000.0,
            clip_gain: 1.0,
            frame_count: 0,
            ..Default::default()
        };
        let inputs = [
            HirariTrackRegionRenderInput {
                region_id: 1,
                ..base
            },
            HirariTrackRegionRenderInput {
                region_id: 2,
                ..base
            },
        ];
        assert_eq!(
            unsafe {
                hirari_track_render_regions(
                    inputs.as_ptr(),
                    inputs.len(),
                    0,
                    FRAMES as u32,
                    destination_left.as_mut_ptr(),
                    destination_right.as_mut_ptr(),
                    0,
                )
            },
            2
        );
        assert!(destination_left[..32].iter().all(|sample| *sample == 0.0));
        assert!(destination_right[..32].iter().all(|sample| *sample == 0.0));
        assert!(destination_left[32..]
            .iter()
            .any(|sample| sample.abs() > 1.0e-6));
        assert!(destination_right[32..]
            .iter()
            .any(|sample| sample.abs() > 1.0e-6));
        let baseline_left = destination_left;
        let baseline_right = destination_right;

        destination_left.fill(0.0);
        destination_right.fill(0.0);
        assert_eq!(
            unsafe {
                hirari_track_render_regions(
                    inputs.as_ptr(),
                    inputs.len(),
                    0,
                    FRAMES as u32,
                    destination_left.as_mut_ptr(),
                    destination_right.as_mut_ptr(),
                    1,
                )
            },
            2
        );
        assert!(destination_left
            .iter()
            .zip(baseline_left)
            .all(|(actual, baseline)| (actual + baseline).abs() <= 1.0e-7));
        assert!(destination_right
            .iter()
            .zip(baseline_right)
            .all(|(actual, baseline)| (actual + baseline).abs() <= 1.0e-7));

        destination_left.fill(0.0);
        destination_right.fill(0.0);
        assert_eq!(
            unsafe {
                hirari_track_render_regions(
                    inputs.as_ptr(),
                    inputs.len(),
                    FRAMES as u64 + 32,
                    FRAMES as u32,
                    destination_left.as_mut_ptr(),
                    destination_right.as_mut_ptr(),
                    0,
                )
            },
            0
        );

        destination_left.fill(1.0);
        destination_right.fill(1.0);
        assert_eq!(
            unsafe {
                hirari_track_render_regions(
                    std::ptr::null(),
                    0,
                    0,
                    FRAMES as u32,
                    destination_left.as_mut_ptr(),
                    destination_right.as_mut_ptr(),
                    1,
                )
            },
            0
        );
        assert!(destination_left.iter().all(|sample| *sample == -1.0));
        assert!(destination_right.iter().all(|sample| *sample == -1.0));
    }
}
