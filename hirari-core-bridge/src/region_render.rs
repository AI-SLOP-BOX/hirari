use crate::audio_note_curve::HirariAudioNoteCurveView;
use crate::region_processing::{HirariRegionBlockConfig, HirariRegionBlockOutput};
use crate::region_warp::HirariWarpMarker;
use std::{cell::UnsafeCell, slice};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionRenderConfig {
    pub source_samples: u64,
    pub source_offset: u64,
    pub source_span: u64,
    pub source_rate: f64,
    pub sample_rate: f64,
    pub base_pitch_ratio: f64,
    pub minimum_delay: f64,
    pub delay_range: f64,
    pub cache_owner: usize,
    pub cache_snapshot: usize,
    pub cache_source: usize,
    pub cache_region_id: u32,
    pub cache_note_segments: usize,
    pub cache_note_segment_count: usize,
    pub cache_pitch_semitones: f32,
    pub sync_group: u32,
    pub reverse: u8,
    pub pitch_preserve_warp: u8,
    pub spectral_stretch_ready: u8,
}

const WSOLA_CACHE_CAPACITY: usize = 128;
const WSOLA_GRAIN_CACHE_CAPACITY: usize = 16;

#[derive(Clone, Copy)]
struct WsolaRegionCache {
    owner: usize,
    snapshot: usize,
    region_id: u32,
    source: usize,
    source_offset: u64,
    source_span: u64,
    source_rate: f64,
    pitch_semitones: f32,
    note_segments: usize,
    note_segment_count: usize,
    reverse: u8,
    valid: bool,
    grain_ids: [u64; WSOLA_GRAIN_CACHE_CAPACITY],
    grain_starts: [f64; WSOLA_GRAIN_CACHE_CAPACITY],
}

impl WsolaRegionCache {
    const EMPTY: Self = Self {
        owner: 0,
        snapshot: 0,
        region_id: 0,
        source: 0,
        source_offset: 0,
        source_span: 0,
        source_rate: 0.0,
        pitch_semitones: 0.0,
        note_segments: 0,
        note_segment_count: 0,
        reverse: 0,
        valid: false,
        grain_ids: [u64::MAX; WSOLA_GRAIN_CACHE_CAPACITY],
        grain_starts: [0.0; WSOLA_GRAIN_CACHE_CAPACITY],
    };
}

/// Determines whether Track's pitch-preserving stretch path is needed for a
/// region. Called while publishing a snapshot, so note/anchor scans stay off
/// the realtime callback.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_needs_spectral_stretch(
    source_rate: f64,
    warp_marker_count: usize,
    pitch_semitones: f32,
    note_curves: *const HirariAudioNoteCurveView,
    note_curve_count: usize,
) -> bool {
    let mut needed = (source_rate - 1.0).abs() > 1.0e-5
        || warp_marker_count != 0
        || pitch_semitones.abs() > 1.0e-4;
    if note_curve_count == 0 {
        return needed;
    }
    if note_curves.is_null() {
        return true;
    }
    let note_curves = slice::from_raw_parts(note_curves, note_curve_count);
    for curve in note_curves {
        if curve.pitch_offset_cents.abs() > 1.0e-3 || curve.formant_offset_cents.abs() > 1.0e-3 {
            needed = true;
            break;
        }
        if curve.anchor_count != 0 && curve.anchors.is_null() {
            return true;
        }
        if curve.anchor_count == 0 {
            continue;
        }
        let anchors = slice::from_raw_parts(curve.anchors, curve.anchor_count);
        if anchors
            .iter()
            .any(|anchor| anchor.pitch_cents.abs() > 1.0e-3 || anchor.formant_cents.abs() > 1.0e-3)
        {
            needed = true;
            break;
        }
    }
    needed
}

thread_local! {
    // Track rendering is single-threaded per callback. This fixed TLS pool
    // preserves grain alignments across blocks without allocation or locks.
    static WSOLA_REGION_CACHES: UnsafeCell<[WsolaRegionCache; WSOLA_CACHE_CAPACITY]> =
        const { UnsafeCell::new([WsolaRegionCache::EMPTY; WSOLA_CACHE_CAPACITY]) };
}

unsafe fn cache_for_render(config: &HirariRegionRenderConfig) -> (*mut u64, *mut f64) {
    let hash = config.cache_owner as u64
        ^ (config.cache_region_id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let index = hash as usize % WSOLA_CACHE_CAPACITY;
    WSOLA_REGION_CACHES.with(|pool| {
        let cache = &mut (*pool.get())[index];
        let matches = cache.valid
            && cache.owner == config.cache_owner
            && cache.snapshot == config.cache_snapshot
            && cache.region_id == config.cache_region_id
            && cache.source == config.cache_source
            && cache.source_offset == config.source_offset
            && cache.source_span == config.source_span
            && cache.source_rate == config.source_rate
            && cache.pitch_semitones == config.cache_pitch_semitones
            && cache.note_segments == config.cache_note_segments
            && cache.note_segment_count == config.cache_note_segment_count
            && cache.reverse == config.reverse;
        if !matches {
            *cache = WsolaRegionCache {
                owner: config.cache_owner,
                snapshot: config.cache_snapshot,
                region_id: config.cache_region_id,
                source: config.cache_source,
                source_offset: config.source_offset,
                source_span: config.source_span,
                source_rate: config.source_rate,
                pitch_semitones: config.cache_pitch_semitones,
                note_segments: config.cache_note_segments,
                note_segment_count: config.cache_note_segment_count,
                reverse: config.reverse,
                valid: true,
                ..WsolaRegionCache::EMPTY
            };
        }
        (
            (*cache).grain_ids.as_mut_ptr(),
            (*cache).grain_starts.as_mut_ptr(),
        )
    })
}

/// Selects and runs the per-frame region DSP path, applies the local formant
/// tilt and adds the gained result into caller-owned Track output buffers.
/// Region iteration, snapshot lifetime, and the optional C++ spectral stretcher
/// remain outside this allocation-free kernel.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_render_block(
    source_left: *const f32,
    source_right: *const f32,
    block_config: *const HirariRegionBlockConfig,
    render_config: *const HirariRegionRenderConfig,
    prepared: *const HirariRegionBlockOutput,
    warp_markers: *const HirariWarpMarker,
    warp_marker_count: usize,
    note_ranges: *const crate::audio_note_curve::HirariAudioNoteSegmentRange,
    note_range_count: usize,
    note_curves: *const HirariAudioNoteCurveView,
    note_curve_count: usize,
    kernel: *const f32,
    window: *const f32,
    spectral_left: *const f32,
    spectral_right: *const f32,
    destination_left: *mut f32,
    destination_right: *mut f32,
    frames: u32,
) -> bool {
    if frames == 0 {
        return true;
    }
    if source_left.is_null()
        || source_right.is_null()
        || block_config.is_null()
        || render_config.is_null()
        || prepared.is_null()
        || kernel.is_null()
        || destination_left.is_null()
        || destination_right.is_null()
    {
        return false;
    }

    let block = &*block_config;
    let render = &*render_config;
    let prepared = &*prepared;
    if block.region_length == 0
        || render.source_samples == 0
        || render.source_span == 0
        || render.source_offset.saturating_add(render.source_span) > render.source_samples
        || !render.base_pitch_ratio.is_finite()
        || render.base_pitch_ratio <= 0.0
        || prepared.gain.is_null()
        || prepared.pitch_cents.is_null()
        || prepared.formant_cents.is_null()
        || prepared.matched_note_indices.is_null()
        || prepared.previous_note_indices.is_null()
        || prepared.source_positions.is_null()
        || prepared.local_source_rates.is_null()
        || (warp_marker_count >= 2 && warp_markers.is_null())
        || (note_range_count != 0
            && (note_ranges.is_null()
                || note_curves.is_null()
                || note_curve_count < note_range_count))
        || (render.spectral_stretch_ready != 0
            && (spectral_left.is_null() || spectral_right.is_null()))
    {
        return false;
    }

    let frames = frames as usize;
    let gains = slice::from_raw_parts(prepared.gain, frames);
    let pitch_cents = slice::from_raw_parts(prepared.pitch_cents, frames);
    let formant_cents = slice::from_raw_parts(prepared.formant_cents, frames);
    let matched_indices = slice::from_raw_parts(prepared.matched_note_indices, frames);
    let previous_indices = slice::from_raw_parts(prepared.previous_note_indices, frames);
    let source_positions = slice::from_raw_parts(prepared.source_positions, frames);
    let local_source_rates = slice::from_raw_parts(prepared.local_source_rates, frames);
    let warp_markers = if warp_marker_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(warp_markers, warp_marker_count)
    };
    let note_ranges = if note_range_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(note_ranges, note_range_count)
    };
    let note_curves = if note_curve_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(note_curves, note_curve_count)
    };
    let destination_left = slice::from_raw_parts_mut(destination_left, frames);
    let destination_right = slice::from_raw_parts_mut(destination_right, frames);
    let spectral_left = if render.spectral_stretch_ready != 0 {
        slice::from_raw_parts(spectral_left, frames)
    } else {
        &[]
    };
    let spectral_right = if render.spectral_stretch_ready != 0 {
        slice::from_raw_parts(spectral_right, frames)
    } else {
        &[]
    };

    let use_wsola = render.spectral_stretch_ready == 0
        && render.pitch_preserve_warp != 0
        && (!warp_markers.is_empty() || (render.source_rate - 1.0).abs() > 1.0e-5);
    if use_wsola && window.is_null() {
        return false;
    }
    let (cache_grain_ids, cache_grain_starts) = if use_wsola {
        cache_for_render(render)
    } else {
        (std::ptr::null_mut(), std::ptr::null_mut())
    };

    let mut frame_output = [0.0f32; 4];
    let mut left = [0.0f32; 2];
    let mut right = [0.0f32; 2];
    for frame in 0..frames {
        let relative = block.region_offset.saturating_add(frame as u64);
        let loop_relative = relative % block.region_length;
        let warped = source_positions[frame];
        if !warped.is_finite() || warped < 0.0 || warped >= render.source_span as f64 {
            continue;
        }
        let cents = pitch_cents[frame];
        let note_pitch_ratio = if cents.is_finite() {
            2.0f64.powf(cents.clamp(-4800.0, 4800.0) / 1200.0)
        } else {
            1.0
        };
        if !note_pitch_ratio.is_finite() || note_pitch_ratio <= 0.0 {
            continue;
        }
        let effective_pitch_ratio = (render.base_pitch_ratio * note_pitch_ratio).clamp(0.25, 4.0);
        let local_source_rate = local_source_rates[frame];
        left.fill(0.0);
        right.fill(0.0);

        if render.spectral_stretch_ready != 0 {
            left[0] = spectral_left[frame];
            right[0] = spectral_right[frame];
        } else if use_wsola {
            frame_output.fill(0.0);
            crate::wsola::hirari_region_wsola_frame(
                source_left,
                source_right,
                render.source_samples,
                render.source_offset,
                render.source_span,
                block.region_length,
                loop_relative,
                render.sample_rate,
                render.source_rate,
                render.base_pitch_ratio,
                effective_pitch_ratio,
                warp_markers.as_ptr(),
                warp_markers.len(),
                note_ranges.as_ptr(),
                note_curves.as_ptr(),
                note_range_count,
                cache_grain_ids,
                cache_grain_starts,
                WSOLA_GRAIN_CACHE_CAPACITY,
                render.sync_group,
                render.reverse,
                kernel,
                window,
                frame_output.as_mut_ptr(),
            );
            left = [frame_output[0], frame_output[1]];
            right = [frame_output[2], frame_output[3]];
        } else if (effective_pitch_ratio - 1.0).abs() > 1.0e-5 {
            let matched_index = matched_indices[frame];
            let previous_index = previous_indices[frame];
            if matched_index >= 0 && matched_index as usize >= note_curves.len()
                || previous_index >= 0 && previous_index as usize >= note_curves.len()
            {
                return false;
            }
            let matched_curve = if matched_index >= 0 {
                note_curves.as_ptr().add(matched_index as usize)
            } else {
                std::ptr::null()
            };
            let previous_curve = if previous_index >= 0 {
                note_curves.as_ptr().add(previous_index as usize)
            } else {
                std::ptr::null()
            };
            let note_seconds = loop_relative as f64 / render.sample_rate.max(1.0);
            frame_output.fill(0.0);
            if !crate::region_pitch_correction::hirari_region_pitch_corrected_frame_with_curves(
                source_left,
                source_right,
                render.source_samples,
                render.source_offset,
                render.source_span,
                warped,
                local_source_rate,
                effective_pitch_ratio,
                render.base_pitch_ratio,
                loop_relative,
                note_seconds,
                render.sample_rate,
                render.minimum_delay,
                render.delay_range,
                render.reverse,
                kernel,
                matched_curve,
                previous_curve,
                frame_output.as_mut_ptr(),
            ) {
                continue;
            }
            left = [frame_output[0], frame_output[1]];
            right = [frame_output[2], frame_output[3]];
        } else {
            crate::region_resampler::hirari_region_read_warped(
                source_left,
                render.source_samples,
                render.source_offset,
                render.source_span,
                warped,
                render.reverse,
                0,
                local_source_rate,
                kernel,
                left.as_mut_ptr(),
            );
            crate::region_resampler::hirari_region_read_warped(
                source_right,
                render.source_samples,
                render.source_offset,
                render.source_span,
                warped,
                render.reverse,
                0,
                local_source_rate,
                kernel,
                right.as_mut_ptr(),
            );
        }

        let mut sample_left = left[0];
        let mut sample_right = right[0];
        if render.spectral_stretch_ready == 0 {
            let formant = if formant_cents[frame].is_finite() {
                formant_cents[frame].clamp(-2400.0, 2400.0)
            } else {
                0.0
            };
            let formant_tilt = (formant / 2400.0) as f32;
            sample_left = (sample_left + left[1] * formant_tilt * 0.5).clamp(-2.0, 2.0);
            sample_right = (sample_right + right[1] * formant_tilt * 0.5).clamp(-2.0, 2.0);
        }
        if sample_left.is_finite() && sample_right.is_finite() {
            destination_left[frame] += sample_left * gains[frame];
            destination_right[frame] += sample_right * gains[frame];
        }
    }
    true
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "region_render_differential_tests.rs"]
mod differential_tests;

#[cfg(test)]
mod cache_tests {
    use super::{cache_for_render, HirariRegionRenderConfig, WSOLA_GRAIN_CACHE_CAPACITY};

    #[test]
    fn wsola_cache_reuses_matching_identity_and_resets_on_every_key_change() {
        let base = HirariRegionRenderConfig {
            source_samples: 4096,
            source_offset: 12,
            source_span: 2048,
            source_rate: 1.25,
            sample_rate: 48_000.0,
            base_pitch_ratio: 1.0,
            cache_owner: 0x1234,
            cache_snapshot: 0x5678,
            cache_source: 0x9abc,
            cache_region_id: 17,
            cache_note_segments: 0xdef0,
            cache_note_segment_count: 3,
            cache_pitch_semitones: 2.0,
            reverse: 1,
            ..HirariRegionRenderConfig::default()
        };
        let reset_and_mark = |config: &HirariRegionRenderConfig| unsafe {
            let (ids, starts) = cache_for_render(config);
            *ids.add(4) = 92;
            *starts.add(4) = 123.5;
        };
        let assert_marked = |config: &HirariRegionRenderConfig, marked: bool| unsafe {
            let (ids, starts) = cache_for_render(config);
            assert_eq!(*ids.add(4), if marked { 92 } else { u64::MAX });
            assert_eq!(*starts.add(4), if marked { 123.5 } else { 0.0 });
        };

        reset_and_mark(&base);
        assert_marked(&base, true);

        let changed = [
            HirariRegionRenderConfig {
                cache_owner: base.cache_owner + 128,
                ..base
            },
            HirariRegionRenderConfig {
                cache_snapshot: base.cache_snapshot + 1,
                ..base
            },
            HirariRegionRenderConfig {
                cache_source: base.cache_source + 1,
                ..base
            },
            HirariRegionRenderConfig {
                source_offset: base.source_offset + 1,
                ..base
            },
            HirariRegionRenderConfig {
                source_span: base.source_span + 1,
                ..base
            },
            HirariRegionRenderConfig {
                source_rate: base.source_rate + 0.25,
                ..base
            },
            HirariRegionRenderConfig {
                cache_pitch_semitones: base.cache_pitch_semitones + 1.0,
                ..base
            },
            HirariRegionRenderConfig {
                cache_note_segments: base.cache_note_segments + 1,
                ..base
            },
            HirariRegionRenderConfig {
                cache_note_segment_count: base.cache_note_segment_count + 1,
                ..base
            },
            HirariRegionRenderConfig { reverse: 0, ..base },
            HirariRegionRenderConfig {
                cache_region_id: base.cache_region_id + 128,
                ..base
            },
        ];
        for config in changed {
            reset_and_mark(&base);
            let _ = unsafe { cache_for_render(&config) };
            assert_marked(&config, false);
        }
        assert_eq!(WSOLA_GRAIN_CACHE_CAPACITY, 16);
    }
}
