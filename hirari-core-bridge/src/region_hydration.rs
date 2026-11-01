//! Project-hydration policy for regions.
//!
//! Two rules used to live inside the native loader: the pre-v42 source-rate
//! migration (older documents stored native source frames as timeline frames)
//! and the range-edit validation that keeps malformed persisted edits away
//! from the real-time renderer. Both are pure numeric policy over the decoded
//! region, so they live here and the loader only applies the result.

use crate::audio_note_curve::HirariAudioNoteAnchor;
use crate::ffi::{NativeNoteSegment, NativeRegionState};
use crate::region_edit_host::HirariRegionState;
use crate::vari_audio::{
    hirari_region_vari_audio_store_clear_segments, hirari_region_vari_audio_store_load,
    HirariAudioNoteSegment, HirariRegionVariAudioStore,
};
use std::slice;

/// Mirrors the engine's warp-marker layout; only `timeline_sample` is rewritten.
#[repr(C)]
pub struct HirariLegacyWarpMarker {
    pub source_sample: u64,
    pub timeline_sample: u64,
    pub transient: bool,
}

/// Mirrors the engine's range-edit layout.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLegacyRangeEdit {
    pub start: u64,
    pub end: u64,
    pub gain: f32,
    pub fade_in: u64,
    pub fade_out: u64,
}

/// Pre-v42 migration inputs, in the order the loader already holds them, plus
/// the outputs it should apply. Marker and range-edit arrays are rewritten in
/// place so the caller keeps ownership of the storage.
#[repr(C)]
pub struct HirariRegionLegacyMigration {
    pub source_offset: u64,
    pub base_source_offset: u64,
    pub source_length: u64,
    pub base_start: u64,
    pub warp_ratio: f64,
    pub loop_count: u32,
    pub fade_in: u64,
    pub fade_out: u64,
    pub decoded_source_rate: f64,
    pub project_sample_rate: f64,
    pub markers: *mut HirariLegacyWarpMarker,
    pub marker_count: usize,
    pub range_edits: *mut HirariLegacyRangeEdit,
    pub range_edit_count: usize,
    pub start: u64,
    pub len: u64,
    pub out_fade_in: u64,
    pub out_fade_out: u64,
    pub source_sample_rate: f64,
}

fn timeline_sample(value: u64, scale: f64) -> u64 {
    let scaled = (value as f64 * scale).round();
    scaled.clamp(0.0, u64::MAX as f64) as u64
}

/// Migrates a pre-v42 region to timeline-frame units. Returns `false` for
/// documents whose geometry cannot be represented, which the loader treats as
/// a hydration failure exactly as it did for the inline version.
unsafe fn migrate(input: &mut HirariRegionLegacyMigration) -> bool {
    let project_rate = input.project_sample_rate;
    let source_rate = input.decoded_source_rate;
    let source_per_timeline = input.warp_ratio * source_rate / project_rate;
    if !source_per_timeline.is_finite() || source_per_timeline <= 0.0 {
        return false;
    }
    let source_trim = input
        .source_offset
        .saturating_sub(input.base_source_offset);
    let migrated_trim = (source_trim as f64 / source_per_timeline).ceil();
    let migrated_length = (input.source_length as f64 / source_per_timeline).ceil();
    if !migrated_trim.is_finite() || !migrated_length.is_finite() {
        return false;
    }
    if migrated_trim > u64::MAX as f64 || migrated_length < 1.0 || migrated_length > u64::MAX as f64
    {
        return false;
    }
    let trim = migrated_trim as u64;
    let length = migrated_length as u64;
    if input.base_start > u64::MAX - trim {
        return false;
    }
    let start = input.base_start + trim;
    let loop_count = u64::from(input.loop_count.max(1));
    if start > u64::MAX - length
        || length > u64::MAX / loop_count
        || start > u64::MAX - length * loop_count
    {
        return false;
    }

    let scale = project_rate / source_rate;
    input.out_fade_in = length.min(timeline_sample(input.fade_in, scale));
    input.out_fade_out = length.min(timeline_sample(input.fade_out, scale));

    if input.marker_count != 0 && !input.markers.is_null() {
        let markers = slice::from_raw_parts_mut(input.markers, input.marker_count);
        for marker in markers {
            marker.timeline_sample = timeline_sample(marker.timeline_sample, scale);
        }
    }
    if input.range_edit_count != 0 && !input.range_edits.is_null() {
        let edits = slice::from_raw_parts_mut(input.range_edits, input.range_edit_count);
        for edit in edits {
            edit.start = timeline_sample(edit.start, scale);
            edit.end = length.min(timeline_sample(edit.end, scale));
            edit.fade_in = timeline_sample(edit.fade_in, scale);
            edit.fade_out = timeline_sample(edit.fade_out, scale);
        }
    }

    input.start = start;
    input.len = length;
    input.source_sample_rate = source_rate;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_legacy_source_rate_migrate(
    input: *mut HirariRegionLegacyMigration,
) -> bool {
    if input.is_null() {
        return false;
    }
    migrate(&mut *input)
}

/// True when a persisted range edit is safe for the renderer: a forward,
/// non-empty span inside the playable length, a bounded gain, and fades that
/// fit inside the span.
fn range_edit_is_valid(edit: &HirariLegacyRangeEdit, playable_length: u64) -> bool {
    if edit.start >= edit.end || edit.end > playable_length {
        return false;
    }
    if !edit.gain.is_finite() || !(0.0..=16.0).contains(&edit.gain) {
        return false;
    }
    let span = edit.end - edit.start;
    edit.fade_in <= span && edit.fade_out <= span
}

/// Filters persisted range edits in place and returns the surviving count.
/// The loader previously ran this predicate twice (once on load and once after
/// the decoded media could shorten the region); one call on the final length
/// produces the same surviving set.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_range_edits_sanitize(
    edits: *mut HirariLegacyRangeEdit,
    count: usize,
    playable_length: u64,
) -> usize {
    if edits.is_null() || count == 0 {
        return 0;
    }
    const MAX_EDITS: usize = 4096;
    let edits = slice::from_raw_parts_mut(edits, count);
    let mut kept = 0_usize;
    for index in 0..edits.len() {
        if kept >= MAX_EDITS {
            break;
        }
        if !range_edit_is_valid(&edits[index], playable_length) {
            continue;
        }
        if kept != index {
            edits[kept] = edits[index];
        }
        kept += 1;
    }
    kept
}

/// Result codes for [`hirari_region_hydrate_decoded`].
pub const HYDRATION_OK: u8 = 0;
pub const HYDRATION_DROP_REGION: u8 = 1;
pub const HYDRATION_FAILED: u8 = 2;

/// Filters the region's persisted range edits in place against its current
/// playable length, keeping at most the renderer's bound.
fn sanitize_native_range_edits(region: &mut NativeRegionState) {
    const MAX_EDITS: usize = 4096;
    let playable = region.sampleLength;
    let mut kept = 0_usize;
    for index in 0..region.rangeEdits.len() {
        if kept >= MAX_EDITS {
            break;
        }
        let edit = region.rangeEdits[index];
        let legacy = HirariLegacyRangeEdit {
            start: edit.start,
            end: edit.end,
            gain: edit.gain,
            fade_in: edit.fadeIn,
            fade_out: edit.fadeOut,
        };
        if !range_edit_is_valid(&legacy, playable) {
            continue;
        }
        region.rangeEdits[kept] = edit;
        kept += 1;
    }
    region.rangeEdits.truncate(kept);
}

/// Completes region hydration once the media is decoded (or known absent).
///
/// The loader used to inline these rules: trim the source length to the
/// available frames, migrate pre-v42 source-frame geometry, and drop range
/// edits that no longer fit. Keeping them here means the loader only copies
/// the decoded record into engine storage.
/// Writes the hydrated region's editable state so the loader installs the
/// record Rust just validated instead of re-mapping every persisted field.
///
/// `state` receives the same field set `HirariRegionState` carries on the C++
/// side; the loader only applies it to the engine's region storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_hydrate_decoded(
    region: *mut NativeRegionState,
    decoded_samples: u64,
    decoded_source_rate: f64,
    project_sample_rate: f64,
    document_version: u32,
    state: *mut HirariRegionState,
) -> u8 {
    if region.is_null() || state.is_null() {
        return HYDRATION_FAILED;
    }
    let region = &mut *region;
    let code = hydrate_decoded(
        region,
        decoded_samples,
        decoded_source_rate,
        project_sample_rate,
        document_version,
    );
    if code == HYDRATION_OK {
        *state = HirariRegionState {
            start: region.samplePosition,
            len: region.sampleLength,
            base_start: region.baseStart,
            base_length: region.baseLength,
            source_offset: region.sourceOffset,
            base_source_offset: region.baseSourceOffset,
            source_length: region.sourceLength,
            fade_in_samples: region.fadeInSamples,
            fade_out_samples: region.fadeOutSamples,
            warp_ratio: region.warpRatio,
            source_sample_rate: f64::from(region.sourceSampleRate),
            timeline_sample_rate: project_sample_rate,
            clip_gain: region.clipGain,
            pitch_semitones: region.pitchSemitones,
            loop_count: region.loopCount,
            sync_group: region.syncGroup,
            muted: region.isMuted,
            reverse: region.reverse,
            locked: region.locked,
            pitch_preserve_warp: region.pitchPreserveWarp,
        };
    }
    code
}

fn hydrate_decoded(
    region: &mut NativeRegionState,
    decoded_samples: u64,
    decoded_source_rate: f64,
    project_sample_rate: f64,
    document_version: u32,
) -> u8 {
    if decoded_samples != 0 {
        if region.sourceOffset >= decoded_samples {
            return HYDRATION_DROP_REGION;
        }
        let available = decoded_samples - region.sourceOffset;
        region.sourceLength = if region.sourceLength == 0 {
            region.sampleLength.min(available)
        } else {
            region.sourceLength.min(available)
        };
        if document_version < 42
            && decoded_source_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&decoded_source_rate)
        {
            // The pre-migration pass mirrors the loader's former first
            // sanitize: edits are filtered against the source-frame length
            // before the migration rescales them.
            sanitize_native_range_edits(region);
            let mut markers: Vec<HirariLegacyWarpMarker> = region
                .warpMarkers
                .iter()
                .map(|marker| HirariLegacyWarpMarker {
                    source_sample: marker.sourceSample,
                    timeline_sample: marker.timelineSample,
                    transient: marker.transient,
                })
                .collect();
            let mut edits: Vec<HirariLegacyRangeEdit> = region
                .rangeEdits
                .iter()
                .map(|edit| HirariLegacyRangeEdit {
                    start: edit.start,
                    end: edit.end,
                    gain: edit.gain,
                    fade_in: edit.fadeIn,
                    fade_out: edit.fadeOut,
                })
                .collect();
            let mut migration = HirariRegionLegacyMigration {
                source_offset: region.sourceOffset,
                base_source_offset: region.baseSourceOffset,
                source_length: region.sourceLength,
                base_start: region.baseStart,
                warp_ratio: region.warpRatio,
                loop_count: region.loopCount,
                fade_in: region.fadeInSamples,
                fade_out: region.fadeOutSamples,
                decoded_source_rate,
                project_sample_rate,
                markers: markers.as_mut_ptr(),
                marker_count: markers.len(),
                range_edits: edits.as_mut_ptr(),
                range_edit_count: edits.len(),
                start: 0,
                len: 0,
                out_fade_in: 0,
                out_fade_out: 0,
                source_sample_rate: 0.0,
            };
            if !unsafe { migrate(&mut migration) } {
                return HYDRATION_FAILED;
            }
            for (marker, migrated) in region.warpMarkers.iter_mut().zip(markers) {
                marker.timelineSample = migrated.timeline_sample;
            }
            for (edit, migrated) in region.rangeEdits.iter_mut().zip(edits) {
                edit.start = migrated.start;
                edit.end = migrated.end;
                edit.gain = migrated.gain;
                edit.fadeIn = migrated.fade_in;
                edit.fadeOut = migrated.fade_out;
            }
            region.samplePosition = migration.start;
            region.sampleLength = migration.len;
            region.fadeInSamples = migration.out_fade_in;
            region.fadeOutSamples = migration.out_fade_out;
            region.sourceSampleRate = migration.source_sample_rate as u32;
        }
    }
    sanitize_native_range_edits(region);
    HYDRATION_OK
}

/// Loads persisted note segments into the Rust-owned VariAudio store,
/// building the flat anchor index the store ABI requires.
pub fn hirari_region_note_store_load_native(
    store: usize,
    segments: &Vec<NativeNoteSegment>,
) -> bool {
    if store == 0 || segments.len() > 4096 {
        return false;
    }
    // An empty persisted table is a successful load, not a clear failure:
    // `clear_segments` reports whether it removed anything.
    let store_ptr = store as *mut HirariRegionVariAudioStore;
    if segments.is_empty() {
        unsafe { hirari_region_vari_audio_store_clear_segments(store_ptr) };
        return true;
    }
    let mut flat_segments: Vec<HirariAudioNoteSegment> = Vec::with_capacity(segments.len());
    let mut flat_anchors: Vec<HirariAudioNoteAnchor> = Vec::new();
    let mut offsets: Vec<usize> = Vec::with_capacity(segments.len());
    let mut counts: Vec<usize> = Vec::with_capacity(segments.len());
    for segment in segments {
        flat_segments.push(HirariAudioNoteSegment {
            start_seconds: segment.startSeconds,
            end_seconds: segment.endSeconds,
            detected_pitch_cents: segment.detectedPitchCents,
            pitch_offset_cents: segment.pitchOffsetCents,
            formant_offset_cents: segment.formantOffsetCents,
        });
        offsets.push(flat_anchors.len());
        counts.push(segment.anchors.len());
        for anchor in &segment.anchors {
            flat_anchors.push(HirariAudioNoteAnchor {
                position_seconds: anchor.positionSeconds,
                pitch_cents: anchor.pitchCents,
                formant_cents: anchor.formantCents,
            });
        }
    }
    unsafe {
        hirari_region_vari_audio_store_load(
            store_ptr,
            flat_segments.as_ptr(),
            flat_segments.len(),
            flat_anchors.as_ptr(),
            offsets.as_ptr(),
            counts.as_ptr(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(timeline: u64) -> HirariLegacyWarpMarker {
        HirariLegacyWarpMarker {
            source_sample: 0,
            timeline_sample: timeline,
            transient: false,
        }
    }

    fn edit(start: u64, end: u64, gain: f32, fade_in: u64, fade_out: u64) -> HirariLegacyRangeEdit {
        HirariLegacyRangeEdit {
            start,
            end,
            gain,
            fade_in,
            fade_out,
        }
    }

    fn migration(
        markers: &mut [HirariLegacyWarpMarker],
        edits: &mut [HirariLegacyRangeEdit],
    ) -> HirariRegionLegacyMigration {
        HirariRegionLegacyMigration {
            source_offset: 2_000,
            base_source_offset: 1_000,
            source_length: 48_000,
            base_start: 10_000,
            warp_ratio: 1.0,
            loop_count: 1,
            fade_in: 480,
            fade_out: 960,
            decoded_source_rate: 48_000.0,
            project_sample_rate: 96_000.0,
            markers: markers.as_mut_ptr(),
            marker_count: markers.len(),
            range_edits: edits.as_mut_ptr(),
            range_edit_count: edits.len(),
            start: 0,
            len: 0,
            out_fade_in: 0,
            out_fade_out: 0,
            source_sample_rate: 0.0,
        }
    }

    #[test]
    fn migrates_duration_trim_and_timeline_units() {
        let mut markers = [marker(240), marker(480)];
        let mut edits = [edit(120, 480, 1.0, 0, 120)];
        let mut values = migration(&mut markers, &mut edits);
        assert!(unsafe { migrate(&mut values) });
        assert_eq!(values.source_sample_rate, 48_000.0);
        // 1000 source frames of trim and 48000 source frames of length, with
        // one source frame per half timeline frame, at twice the timeline rate.
        assert_eq!(values.start, 12_000);
        assert_eq!(values.len, 96_000);
        assert_eq!(values.out_fade_in, 960);
        assert_eq!(values.out_fade_out, 1_920);
        assert_eq!(markers[0].timeline_sample, 480);
        assert_eq!(markers[1].timeline_sample, 960);
        assert_eq!(edits[0].end, 960);
    }

    #[test]
    fn rate_ratio_scales_timeline_values() {
        let mut markers = [marker(300)];
        let mut edits = [];
        let mut values = migration(&mut markers, &mut edits);
        values.source_sample_rate = 0.0;
        values.decoded_source_rate = 96_000.0;
        values.project_sample_rate = 48_000.0;
        values.warp_ratio = 1.0;
        assert!(unsafe { migrate(&mut values) });
        // Source frames are twice as long as timeline frames here.
        assert_eq!(values.len, 24_000);
        assert_eq!(markers[0].timeline_sample, 150);
        assert_eq!(values.out_fade_in, 240);
    }

    #[test]
    fn rejects_unrepresentable_geometry() {
        let mut markers = [];
        let mut edits = [];
        let mut values = migration(&mut markers, &mut edits);
        values.source_length = u64::MAX;
        values.decoded_source_rate = 1.0;
        values.project_sample_rate = 384_000.0;
        assert!(!unsafe { migrate(&mut values) });

        let mut values = migration(&mut markers, &mut edits);
        values.warp_ratio = 0.0;
        assert!(!unsafe { migrate(&mut values) });

        let mut values = migration(&mut markers, &mut edits);
        values.decoded_source_rate = f64::NAN;
        assert!(!unsafe { migrate(&mut values) });
    }

    #[test]
    fn sanitize_keeps_only_renderable_edits() {
        let mut edits = [
            edit(0, 100, 1.0, 0, 0),
            edit(100, 100, 1.0, 0, 0),
            edit(0, 200, 1.0, 0, 0),
            edit(0, 100, f32::NAN, 0, 0),
            edit(0, 100, -1.0, 0, 0),
            edit(0, 100, 17.0, 0, 0),
            edit(0, 100, 1.0, 101, 0),
            edit(0, 100, 1.0, 0, 101),
            edit(10, 50, 0.5, 10, 10),
        ];
        let kept = unsafe {
            hirari_region_range_edits_sanitize(edits.as_mut_ptr(), edits.len(), 100)
        };
        assert_eq!(kept, 2);
        assert_eq!((edits[0].start, edits[0].end), (0, 100));
        assert_eq!((edits[1].start, edits[1].end), (10, 50));
    }

    #[test]
    fn sanitize_caps_entries_and_handles_empty_input() {
        let mut edits = vec![edit(0, 10, 1.0, 0, 0); 4_100];
        let kept = unsafe {
            hirari_region_range_edits_sanitize(edits.as_mut_ptr(), edits.len(), 10)
        };
        assert_eq!(kept, 4096);
        assert_eq!(unsafe { hirari_region_range_edits_sanitize(std::ptr::null_mut(), 4, 10) }, 0);
    }

    #[test]
    fn sanitize_drops_edits_beyond_a_shrunk_region() {
        let mut edits = [edit(0, 90, 1.0, 0, 0), edit(0, 400, 1.0, 0, 0)];
        let kept = unsafe {
            hirari_region_range_edits_sanitize(edits.as_mut_ptr(), edits.len(), 100)
        };
        assert_eq!(kept, 1);
        assert_eq!(edits[0].end, 90);
    }
}
