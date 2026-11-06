//! Builds the native `ARUA` project state from the borrowed layout snapshot.
//!
//! C++ used to walk the whole model a second time in `saveProject`, filling an
//! identical set of encoder records. It now gathers the same snapshot the
//! layout JSON writer consumes (plus the save-only GUI blobs), and this module
//! maps that snapshot onto the encoder types. Every field rule - catalogue
//! bounds, little-endian plug-in ids, store materialisation - lives here.

use crate::ffi::{NativeAutomationPoint, NativeBlob, NativeNoteAnchor, NativeNoteSegment, NativePluginAutomationLane, NativeProcessingStep, NativeProjectState, NativeRangeEdit, NativeRegionState, NativeRouteState, NativeSidechainState, NativeTempoEventState, NativeTrackState, NativeWarpMarker, };
use crate::AudioEngine;
use crate::HirariUnifiedEngine;
use crate::project_layout::{
    HirariLayoutAutomationPoint, HirariLayoutByteBlock, HirariLayoutPluginLane,
    HirariLayoutRegion, HirariLayoutString, HirariLayoutTrack, HirariProjectLayoutSnapshot,
};
use crate::region_processing_history::{HirariEventStepView, HirariRegionProcessingHistoryStore};
use crate::vari_audio::{HirariAudioNoteSegment, HirariRegionVariAudioStore};
use crate::audio_note_curve::HirariAudioNoteAnchor;
use std::ffi::c_void;

const PROJECT_VERSION: u32 = 42;
const MAX_NOTE_SEGMENTS: usize = 4096;
const MAX_NOTE_ANCHORS: usize = MAX_NOTE_SEGMENTS * 4096;
const MAX_HISTORY_STEPS: usize = 4096;
const MAX_HISTORY_BYTES: usize = 1_048_576;

fn slice_of<T>(data: *const T, count: usize) -> Option<&'static [T]> {
    if count == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, count) })
}

fn layout_string(value: HirariLayoutString) -> Option<String> {
    let bytes = slice_of(value.data, value.len)?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn block_bytes(block: &HirariLayoutByteBlock) -> Option<&'static [u8]> {
    slice_of(block.bytes, block.count)
}

fn blobs(blocks: &[HirariLayoutByteBlock]) -> Option<Vec<NativeBlob>> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        out.push(NativeBlob {
            bytes: block_bytes(block)?.to_vec(),
        });
    }
    Some(out)
}

fn points(
    data: *const HirariLayoutAutomationPoint,
    count: usize,
) -> Option<Vec<NativeAutomationPoint>> {
    let source = slice_of(data, count)?;
    Some(
        source
            .iter()
            .map(|point| NativeAutomationPoint {
                time: point.time,
                value: point.value,
                curve: point.curve,
            })
            .collect(),
    )
}

fn plugin_automation(lanes: &[HirariLayoutPluginLane]) -> Option<Vec<NativePluginAutomationLane>> {
    let mut out = Vec::with_capacity(lanes.len());
    for lane in lanes {
        out.push(NativePluginAutomationLane {
            pluginIndex: lane.plugin_index,
            parameterId: lane.parameter_id,
            points: points(lane.points, lane.point_count)?,
        });
    }
    Some(out)
}

unsafe fn region_notes(
    store: *const HirariRegionVariAudioStore,
) -> Option<Vec<NativeNoteSegment>> {
    if store.is_null() {
        return Some(Vec::new());
    }
    let (mut segment_count, mut anchor_total) = (0usize, 0usize);
    if !unsafe {
        crate::vari_audio::hirari_region_vari_audio_store_sizes(
            store,
            &mut segment_count,
            &mut anchor_total,
        )
    } {
        return None;
    }
    if segment_count > MAX_NOTE_SEGMENTS || anchor_total > MAX_NOTE_ANCHORS {
        return None;
    }
    if segment_count == 0 {
        return Some(Vec::new());
    }
    let mut segments = vec![HirariAudioNoteSegment::default(); segment_count];
    let mut anchors = vec![HirariAudioNoteAnchor::default(); anchor_total];
    let mut offsets = vec![0usize; segment_count];
    let mut counts = vec![0usize; segment_count];
    let (mut written_segments, mut written_anchors) = (0usize, 0usize);
    if !unsafe {
        crate::vari_audio::hirari_region_vari_audio_store_materialize(
            store,
            segments.as_mut_ptr(),
            segments.len(),
            anchors.as_mut_ptr(),
            anchors.len(),
            offsets.as_mut_ptr(),
            counts.as_mut_ptr(),
            &mut written_segments,
            &mut written_anchors,
        )
    } {
        return None;
    }
    let mut out = Vec::with_capacity(written_segments.min(segments.len()));
    for index in 0..written_segments.min(segments.len()) {
        let segment = segments[index];
        let start = offsets[index];
        let count = counts[index];
        if start > written_anchors || count > written_anchors - start {
            return None;
        }
        out.push(NativeNoteSegment {
            startSeconds: segment.start_seconds,
            endSeconds: segment.end_seconds,
            detectedPitchCents: segment.detected_pitch_cents,
            pitchOffsetCents: segment.pitch_offset_cents,
            formantOffsetCents: segment.formant_offset_cents,
            anchors: anchors[start..start + count]
                .iter()
                .map(|anchor| NativeNoteAnchor {
                    positionSeconds: anchor.position_seconds,
                    pitchCents: anchor.pitch_cents,
                    formantCents: anchor.formant_cents,
                })
                .collect(),
        });
    }
    Some(out)
}

unsafe fn region_history(
    store: *const HirariRegionProcessingHistoryStore,
) -> Option<Vec<NativeProcessingStep>> {
    if store.is_null() {
        return Some(Vec::new());
    }
    let (mut count, mut byte_total) = (0usize, 0usize);
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_sizes(
            store,
            &mut count,
            &mut byte_total,
        )
    } {
        return None;
    }
    if count > MAX_HISTORY_STEPS || byte_total > MAX_HISTORY_BYTES {
        return None;
    }
    if count == 0 {
        return Some(Vec::new());
    }
    let mut views = vec![HirariEventStepView::default(); count];
    let mut bytes = vec![0u8; byte_total];
    let (mut written_count, mut written_bytes) = (0usize, 0usize);
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_materialize(
            store,
            views.as_mut_ptr(),
            views.len(),
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut written_count,
            &mut written_bytes,
        )
    } {
        return None;
    }
    let mut out = Vec::with_capacity(written_count.min(views.len()));
    for view in views.iter().take(written_count.min(views.len())) {
        if view.operation_offset > written_bytes
            || view.operation_len > written_bytes - view.operation_offset
        {
            return None;
        }
        let operation = std::str::from_utf8(
            &bytes[view.operation_offset..view.operation_offset + view.operation_len],
        )
        .ok()?;
        out.push(NativeProcessingStep {
            id: view.id,
            operation: operation.to_owned(),
            parameter: view.parameter,
            enabled: view.enabled,
        });
    }
    Some(out)
}

fn warp_markers(region: &HirariLayoutRegion) -> Option<Vec<NativeWarpMarker>> {
    let markers = slice_of(region.warp_markers, region.warp_marker_count)?;
    Some(
        markers
            .iter()
            .map(|marker| NativeWarpMarker {
                sourceSample: marker.source_sample,
                timelineSample: marker.timeline_sample,
                transient: marker.transient != 0,
            })
            .collect(),
    )
}

fn range_edits(region: &HirariLayoutRegion) -> Option<Vec<NativeRangeEdit>> {
    let edits = slice_of(region.range_edits, region.range_edit_count)?;
    Some(
        edits
            .iter()
            .map(|edit| NativeRangeEdit {
                start: edit.start,
                end: edit.end,
                gain: edit.gain,
                fadeIn: edit.fade_in,
                fadeOut: edit.fade_out,
            })
            .collect(),
    )
}

fn region_state(region: &HirariLayoutRegion, track_id: u32) -> Option<NativeRegionState> {
    let state = region.state;
    Some(NativeRegionState {
        id: region.id,
        trackId: track_id,
        samplePosition: state.start,
        sampleLength: state.len,
        sourceLength: state.source_length,
        sourceSampleRate: region.source_sample_rate,
        sourceOffset: state.source_offset,
        baseStart: state.base_start,
        baseSourceOffset: state.base_source_offset,
        baseLength: state.base_length,
        isMuted: state.muted,
        filePath: layout_string(region.path)?,
        name: layout_string(region.name)?,
        clipGain: state.clip_gain,
        fadeInSamples: state.fade_in_samples,
        fadeOutSamples: state.fade_out_samples,
        reverse: state.reverse,
        warpRatio: state.warp_ratio,
        pitchPreserveWarp: state.pitch_preserve_warp,
        pitchSemitones: state.pitch_semitones,
        loopCount: state.loop_count,
        locked: state.locked,
        syncGroup: state.sync_group,
        rangeEdits: range_edits(region)?,
        processingHistory: unsafe { region_history(region.processing_store) }?,
        audioNoteSegments: unsafe { region_notes(region.note_store) }?,
        warpMarkers: warp_markers(region)?,
    })
}

fn track_state(track: &HirariLayoutTrack) -> Option<NativeTrackState> {
    let plugin_types = slice_of(track.plugin_types, track.plugin_type_count)?;
    let mut plugin_data = Vec::with_capacity(plugin_types.len() * 4);
    for plugin_type in plugin_types {
        plugin_data.extend_from_slice(&plugin_type.to_le_bytes());
    }
    let plugin_bypass = slice_of(track.plugin_bypass, track.plugin_type_count)?;
    let plugin_states = slice_of(track.plugin_states, track.plugin_state_count)?;
    let sandbox_paths = slice_of(track.sandbox_plugin_paths, track.sandbox_plugin_path_count)?;
    let mut sandboxed_paths = Vec::with_capacity(sandbox_paths.len());
    for path in sandbox_paths {
        sandboxed_paths.push(layout_string(*path)?);
    }
    let sandbox_states = slice_of(track.sandbox_plugin_states, track.sandbox_plugin_state_count)?;
    let gui_states = slice_of(track.plugin_gui_states, track.plugin_gui_state_count)?;
    let lanes = slice_of(track.plugin_automation, track.plugin_automation_count)?;
    Some(NativeTrackState {
        id: track.id,
        trackType: track.track_type,
        volume: track.volume,
        pan: track.pan,
        pan3d_x: track.spatial_x,
        pan3d_y: track.spatial_y,
        pan3d_z: track.spatial_z,
        muted: track.muted,
        solo: track.solo,
        phaseInvert: track.phase_inverted,
        recordArmed: track.record_armed,
        trackDelaySamples: track.track_delay_samples.min(u64::from(u32::MAX)) as u32,
        pluginName: layout_string(track.name)?,
        pluginData: plugin_data,
        pluginTypes: plugin_types.to_vec(),
        pluginStates: blobs(plugin_states)?,
        pluginGuiStates: blobs(gui_states)?,
        pluginBypass: plugin_bypass.to_vec(),
        sandboxedPluginPaths: sandboxed_paths,
        sandboxedPluginStates: blobs(sandbox_states)?,
        volumeAutomation: points(track.volume_automation, track.volume_automation_count)?,
        panAutomation: points(track.pan_automation, track.pan_automation_count)?,
        trackDelayAutomation: points(
            track.track_delay_automation,
            track.track_delay_automation_count,
        )?,
        pluginAutomation: plugin_automation(lanes)?,
    })
}

fn project_routes(routing_state: *const c_void) -> Option<Vec<NativeRouteState>> {
    let mut count = 0usize;
    let records = unsafe {
        crate::routing_engine_core::hirari_routing_engine_copy_connections(routing_state, &mut count)
    };
    let slice = slice_of(records, count)?;
    let routes = slice
        .iter()
        .map(|record| NativeRouteState {
            sourceId: record.source_id,
            destinationId: record.destination_id,
            gain: record.gain,
            send: record.send,
            preFader: record.pre_fader,
        })
        .collect();
    unsafe { crate::routing_engine_core::hirari_routing_connection_array_free(records, count) };
    Some(routes)
}

fn project_tempo(tempo_state: *const c_void) -> Vec<NativeTempoEventState> {
    unsafe { crate::tempo::project_tempo_events(tempo_state) }
        .into_iter()
        .map(|(sample, bpm, ramp)| NativeTempoEventState {
            sample,
            bpm,
            ramp: u8::from(ramp),
        })
        .collect()
}

fn build_state(
    snapshot: &HirariProjectLayoutSnapshot,
    tempo_state: *const c_void,
    routing_state: *const c_void,
    bpm: f64,
    root_note: i32,
    scale_type: i32,
) -> Option<NativeProjectState> {
    let tracks = slice_of(snapshot.tracks, snapshot.track_count)?;
    let mut state = NativeProjectState::default();
    state.version = PROJECT_VERSION;
    state.bpm = bpm;
    state.rootNote = root_note;
    state.scaleType = scale_type;
    state.sampleRate = snapshot.timeline_sample_rate.round() as u32;
    for track in tracks {
        state.tracks.push(track_state(track)?);
        let regions = slice_of(track.regions, track.region_count)?;
        for region in regions {
            state.regions.push(region_state(region, track.id)?);
        }
    }
    let sidechains = slice_of(snapshot.sidechain_routes, snapshot.sidechain_route_count)?;
    state.sidechains = sidechains
        .iter()
        .map(|route| NativeSidechainState {
            sourceId: route.source_id,
            destinationId: route.destination_id,
            pluginIndex: route.plugin_index,
            tapPoint: route.tap_point,
        })
        .collect();
    state.routes = project_routes(routing_state)?;
    state.tempoEvents = project_tempo(tempo_state);
    Some(state)
}

/// Encodes the borrowed layout snapshot as an `ARUA` project file.
///
/// # Safety
/// `snapshot` must point at a live snapshot whose pointers stay valid for the
/// call; `tempo_state`/`routing_state` must be null or live engine handles;
/// `path` must be readable for `path_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_save_from_layout(
    path: *const u8,
    path_len: usize,
    snapshot: *const HirariProjectLayoutSnapshot,
    tempo_state: *const c_void,
    routing_state: *const c_void,
    arrangement_document: usize,
    bpm: f64,
    root_note: i32,
    scale_type: i32,
) -> bool {
    let Some(path_bytes) = slice_of(path, path_len) else {
        return false;
    };
    let Ok(path) = std::str::from_utf8(path_bytes) else {
        return false;
    };
    if path.is_empty() || snapshot.is_null() {
        return false;
    }
    let Some(state) = build_state(
        unsafe { &*snapshot },
        tempo_state,
        routing_state,
        bpm,
        root_note,
        scale_type,
    ) else {
        return false;
    };
    crate::native_project::native_project_save_with_arrangement(path, &state, arrangement_document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_null_snapshot_and_empty_path() {
        let path = b"/tmp/hirari-save-layout-test.hirari";
        assert!(!unsafe {
            hirari_project_save_from_layout(
                path.as_ptr(),
                path.len(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                120.0,
                0,
                0,
            )
        });
        let snapshot = HirariProjectLayoutSnapshot::default();
        assert!(!unsafe {
            hirari_project_save_from_layout(
                path.as_ptr(),
                0,
                &snapshot,
                std::ptr::null(),
                std::ptr::null(),
                0,
                120.0,
                0,
                0,
            )
        });
    }

    #[test]
    fn builds_empty_state_with_bounded_sample_rate() {
        let snapshot = HirariProjectLayoutSnapshot {
            timeline_sample_rate: 48_000.0,
            ..Default::default()
        };
        let state = build_state(&snapshot, std::ptr::null(), std::ptr::null(), 120.0, 0, 0)
            .expect("empty snapshot");
        assert_eq!(state.version, PROJECT_VERSION);
        assert_eq!(state.sampleRate, 48_000);
        assert!(state.tracks.is_empty());
        assert!(state.regions.is_empty());
        assert!(state.routes.is_empty());
        assert!(state.tempoEvents.is_empty());
    }
}
