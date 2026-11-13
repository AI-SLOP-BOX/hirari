//! Scalar-edit decision engine (min/max/clamps and per-field decide rules),
//! split out of region_processing.rs.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionScalarDecision {
    pub accepted: bool,
    /// Clamped value the caller must store. LoopCount and SyncGroup carry their
    /// integer payload as the raw `f32` bit pattern of this field.
    pub value_a: f32,
    /// Clamped fade-out. Only meaningful for `HirariRegionScalarField::Fades`.
    pub value_b: f32,
}

impl HirariRegionScalarDecision {
    fn reject() -> Self {
        Self {
            accepted: false,
            value_a: 0.0,
            value_b: 0.0,
        }
    }

    fn accept(value_a: f32, value_b: f32) -> Self {
        Self {
            accepted: true,
            value_a,
            value_b,
        }
    }

    pub(crate) fn decide(edit: &HirariRegionScalarEdit) -> Self {
        // Clip gain and the fade pair clamp into range; the other scalar fields
        // reject out-of-range input outright so callers keep their own bounds.
        //
        // Non-finite input is rejected rather than clamped, matching the
        // previous C++ `std::isfinite` guards, which ran before any clamp.
        //
        // A locked region rejects every field. Editing the lock itself is the
        // only permitted mutation on a locked region, and it never comes
        // through here.
        match edit.field {
            HirariRegionScalarField::ClipGain => {
                if !edit.value_a.is_finite() || edit.locked {
                    return Self::reject();
                }
                Self::accept(edit.value_a.clamp(0.0, 2.0), 0.0)
            }
            HirariRegionScalarField::Muted | HirariRegionScalarField::Reverse => {
                if edit.locked {
                    return Self::reject();
                }
                // Booleans carry no payload; the caller stores its own `bool`.
                Self::accept(0.0, 0.0)
            }
            HirariRegionScalarField::PitchSemitones => {
                if !edit.value_a.is_finite() || edit.locked || edit.value_a.abs() > 24.0 {
                    return Self::reject();
                }
                Self::accept(edit.value_a, 0.0)
            }
            HirariRegionScalarField::LoopCount => {
                if edit.locked {
                    return Self::reject();
                }
                let count = edit.value_a.to_bits();
                if count == 0 || count > 1024 {
                    return Self::reject();
                }
                // `len * count` and `start + len * count` must both stay inside
                // u64, otherwise the rendered loop length would wrap around.
                let Some(total) = edit.region_len.checked_mul(u64::from(count)) else {
                    return Self::reject();
                };
                if edit.region_start > u64::MAX - total {
                    return Self::reject();
                }
                Self::accept(edit.value_a, 0.0)
            }
            HirariRegionScalarField::SyncGroup => {
                if edit.locked {
                    return Self::reject();
                }
                // An unrestricted u32 tag carried verbatim in `value_a`.
                Self::accept(edit.value_a, 0.0)
            }
            // The lock is the one field a locked region may still change, so
            // it deliberately ignores `edit.locked`.
            HirariRegionScalarField::Locked => Self::accept(0.0, 0.0),
            HirariRegionScalarField::PitchPreserveWarp => {
                if edit.locked {
                    return Self::reject();
                }
                Self::accept(0.0, 0.0)
            }
            HirariRegionScalarField::Fades => {
                if !edit.value_a.is_finite() || !edit.value_b.is_finite() || edit.locked {
                    return Self::reject();
                }
                Self::accept(edit.value_a.clamp(0.0, 1.0), edit.value_b.clamp(0.0, 1.0))
            }
        }
    }
}

/// Authoritative policy for scalar region edits.
///
/// Both the C++ storage layer (`Track`) and the C++ undo layer
/// (`HirariUnifiedEngine`) call this, so neither can drift from the other.
///
/// The decision is written through an out-param rather than returned by value
/// because returning a C++ struct is not a C-compatible calling convention and
/// this boundary is plain C ABI. Returns false and writes nothing when `out` is
/// null, so a C++ caller that forgets to allocate the out-param cannot silently
/// read an uninitialized decision as "accepted".
#[no_mangle]
pub unsafe extern "C" fn hirari_region_scalar_edit_decision(
    edit: HirariRegionScalarEdit,
    out: *mut HirariRegionScalarDecision,
) -> bool {
    if out.is_null() {
        return false;
    }
    let decision = HirariRegionScalarDecision::decide(&edit);
    unsafe { out.write(decision) };
    true
}

/// Converts a normalized fade (0..1) into a sample count for `region_len`.
///
/// Truncates rather than rounds, matching the previous C++
/// `static_cast<uint64_t>(norm * len)`, and clamps to `region_len` so a fade can
/// never exceed the region it belongs to. Computed in `f64` so the multiply
/// cannot lose precision for very large regions.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_fade_samples(fade_norm: f32, region_len: u64) -> u64 {
    if !fade_norm.is_finite() || fade_norm <= 0.0 {
        return 0;
    }
    let norm = f64::from(fade_norm.clamp(0.0, 1.0));
    let samples = (norm * region_len as f64) as u64;
    samples.min(region_len)
}

#[cfg(test)]
#[path = "region_scalar_edit_policy_tests.rs"]
mod scalar_edit_policy_tests;





#[cfg(test)]
#[path = "region_overlap_tests.rs"]
mod overlap_tests;


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
#[path = "region_chunk_render_tests.rs"]
mod chunk_render_tests;


