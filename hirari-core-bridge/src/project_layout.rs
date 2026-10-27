/// Advances the project-layout FNV fingerprint for one stream chunk.
///
/// # Safety
/// `state` must be a writable `u64`; when `length` is nonzero `data` must be
/// readable for that many bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_fingerprint_update(
    state: *mut u64,
    data: *const u8,
    length: usize,
) {
    if state.is_null() || (length != 0 && data.is_null()) {
        return;
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    let hash = unsafe { &mut *state };
    for byte in bytes {
        *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
}

/// Builds a cheap polling token that includes layout, MIDI note edits, and
/// control changes while keeping the combination arithmetic Rust-owned.
#[no_mangle]
pub extern "C" fn hirari_project_state_revision_mix(
    layout: u64,
    midi: u64,
    control: u64,
) -> u64 {
    let mut value = layout ^ (midi.wrapping_add(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(layout << 6).wrapping_add(layout >> 2));
    value ^= control.wrapping_add(0x517c_c1b7_2722_0a95)
        .wrapping_add(value << 6).wrapping_add(value >> 2);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

// ============================================================
// Project-layout JSON serialization.
//
// This bytes-for-bytes replaces the control-side C++ stringstream writer.
// C++ still gathers its model into a borrowed snapshot; every ordering,
// escaping, number-formatting and structural rule now lives here. The call
// is synchronous: all pointers in the snapshot are valid only until it
// returns, and the JSON buffer is allocated by Rust and released through
// `hirari_project_layout_json_free`.
// ============================================================

use crate::audio_note_curve::HirariAudioNoteAnchor;
use crate::region_edit_host::HirariRegionState;
use crate::region_processing::NativeRangeEdit;
use crate::region_processing_history::{HirariEventStepView, HirariRegionProcessingHistoryStore};
use crate::vari_audio::{
    hirari_region_vari_audio_store_materialize, hirari_region_vari_audio_store_sizes,
    HirariAudioNoteSegment, HirariRegionVariAudioStore, HirariVariAudioFrozen,
};
use crate::region_warp::HirariWarpMarker;
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutString {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutAutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curve: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutPluginLane {
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub points: *const HirariLayoutAutomationPoint,
    pub point_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutParameterBlock {
    pub values: *const f32,
    pub count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutByteBlock {
    pub bytes: *const u8,
    pub count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutSidechainRoute {
    pub source_id: u32,
    pub destination_id: u32,
    pub plugin_index: u32,
    pub tap_point: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutFeedbackRoute {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
}

#[repr(C)]
#[derive(Default)]
pub struct HirariLayoutRegion {
    pub id: u32,
    pub name: HirariLayoutString,
    pub state: HirariRegionState,
    pub source_sample_rate: u32,
    pub path: HirariLayoutString,
    pub missing: bool,
    pub audio_loaded: bool,
    pub warp_markers: *const HirariWarpMarker,
    pub warp_marker_count: usize,
    pub range_edits: *const NativeRangeEdit,
    pub range_edit_count: usize,
    pub note_store: *const HirariRegionVariAudioStore,
    pub note_frozen: *const HirariVariAudioFrozen,
    pub processing_store: *const HirariRegionProcessingHistoryStore,
}

#[repr(C)]
#[derive(Default)]
pub struct HirariLayoutTrack {
    pub id: u32,
    pub name: HirariLayoutString,
    pub track_type: u32,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub record_armed: bool,
    pub phase_inverted: bool,
    pub track_delay_samples: u64,
    pub frozen: bool,
    pub frozen_total_samples: u64,
    pub frozen_sample_rate: u32,
    pub frozen_path: HirariLayoutString,
    pub volume_automation: *const HirariLayoutAutomationPoint,
    pub volume_automation_count: usize,
    pub pan_automation: *const HirariLayoutAutomationPoint,
    pub pan_automation_count: usize,
    pub track_delay_automation: *const HirariLayoutAutomationPoint,
    pub track_delay_automation_count: usize,
    pub expression_map_json: HirariLayoutString,
    pub expression_map_pro_json: HirariLayoutString,
    pub plugin_automation: *const HirariLayoutPluginLane,
    pub plugin_automation_count: usize,
    pub plugin_types: *const u32,
    pub plugin_type_count: usize,
    pub plugin_bypass: *const u8,
    pub plugin_parameters: *const HirariLayoutParameterBlock,
    pub plugin_states: *const HirariLayoutByteBlock,
    pub plugin_state_count: usize,
    pub sandbox_plugin_paths: *const HirariLayoutString,
    pub sandbox_plugin_path_count: usize,
    pub sandbox_plugin_states: *const HirariLayoutByteBlock,
    pub sandbox_plugin_state_count: usize,
    pub regions: *const HirariLayoutRegion,
    pub region_count: usize,
}

#[repr(C)]
#[derive(Default)]
pub struct HirariProjectLayoutSnapshot {
    pub tracks: *const HirariLayoutTrack,
    pub track_count: usize,
    pub sidechain_routes: *const HirariLayoutSidechainRoute,
    pub sidechain_route_count: usize,
    pub feedback_routes: *const HirariLayoutFeedbackRoute,
    pub feedback_route_count: usize,
    pub timeline_sample_rate: f64,
}

struct LayoutWriter<'a> {
    out: Vec<u8>,
    cancel: Option<(unsafe extern "C" fn(*const c_void) -> bool, *const c_void)>,
    sidechain_routes: &'a [HirariLayoutSidechainRoute],
    feedback_routes: &'a [HirariLayoutFeedbackRoute],
}

impl LayoutWriter<'_> {
    fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|(is_cancelled, context)| unsafe { is_cancelled(context) })
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn text(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'"' => self.out.extend_from_slice(b"\\\""),
                b'\\' => self.out.extend_from_slice(b"\\\\"),
                b'\n' => self.out.extend_from_slice(b"\\n"),
                b'\r' => self.out.extend_from_slice(b"\\r"),
                b'\t' => self.out.extend_from_slice(b"\\t"),
                control if control < 0x20 => self.out.push(b'?'),
                other => self.out.push(other),
            }
        }
    }

    fn number(&mut self, value: f64) {
        let text = if value.is_finite() {
            format!("{value:.6}")
        } else {
            String::from("0")
        };
        self.raw(text.as_bytes());
    }

    fn number32(&mut self, value: f32) {
        self.number(f64::from(value));
    }

    fn parameter(&mut self, value: f32) {
        let text = if value.is_finite() {
            format!("{value:.6}")
        } else {
            String::from("0.0")
        };
        self.raw(text.as_bytes());
    }

    fn boolean(&mut self, value: bool) {
        self.raw(if value { b"true" } else { b"false" });
    }

    fn hex(&mut self, bytes: &[u8]) {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for &byte in bytes {
            self.out.push(DIGITS[(byte >> 4) as usize]);
            self.out.push(DIGITS[(byte & 0x0f) as usize]);
        }
    }
}

unsafe fn optional_slice<'a, T>(data: *const T, count: usize) -> Option<&'a [T]> {
    if count == 0 {
        Some(&[])
    } else if data.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(data, count) })
    }
}

unsafe fn string_bytes<'a>(value: &HirariLayoutString) -> Option<&'a [u8]> {
    unsafe { optional_slice(value.data, value.len) }
}

fn write_automation_points(
    writer: &mut LayoutWriter,
    points: &[HirariLayoutAutomationPoint],
) -> bool {
    writer.raw(b"[");
    for (index, point) in points.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"time\":");
        writer.number(point.time);
        writer.raw(b",\"value\":");
        writer.number32(point.value);
        writer.raw(b",\"curve\":");
        writer.number32(point.curve);
        writer.raw(b"}");
    }
    writer.raw(b"]");
    true
}

fn write_note_segment(
    writer: &mut LayoutWriter,
    start_seconds: f64,
    end_seconds: f64,
    detected_pitch_cents: f64,
    pitch_offset_cents: f64,
    formant_offset_cents: f64,
    anchors: &[HirariAudioNoteAnchor],
) -> bool {
    writer.raw(b"{\"start\":");
    writer.number(start_seconds);
    writer.raw(b",\"end\":");
    writer.number(end_seconds);
    writer.raw(b",\"detected_pitch_cents\":");
    writer.number(detected_pitch_cents);
    writer.raw(b",\"pitch_offset_cents\":");
    writer.number(pitch_offset_cents);
    writer.raw(b",\"formant_offset_cents\":");
    writer.number(formant_offset_cents);
    writer.raw(b",\"anchors\":[");
    for (index, anchor) in anchors.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"position\":");
        writer.number(anchor.position_seconds);
        writer.raw(b",\"pitch_cents\":");
        writer.number(anchor.pitch_cents);
        writer.raw(b",\"formant_cents\":");
        writer.number(anchor.formant_cents);
        writer.raw(b"}");
    }
    writer.raw(b"]}");
    true
}

unsafe fn write_region_note_segments(
    writer: &mut LayoutWriter,
    region: &HirariLayoutRegion,
) -> bool {
    if region.note_store.is_null() {
        return false;
    }
    writer.raw(b"[");
    let mut first = true;
    if !region.note_frozen.is_null() {
        let frozen = unsafe { &*region.note_frozen };
        for (curve, range) in frozen.segment_views() {
            if writer.cancelled() {
                return false;
            }
            let anchors =
                match unsafe { optional_slice(curve.anchors, curve.anchor_count) } {
                    Some(anchors) => anchors,
                    None => return false,
                };
            if !first {
                writer.raw(b",");
            }
            first = false;
            if !write_note_segment(
                writer,
                curve.start_seconds,
                curve.end_seconds,
                range.detected_pitch_cents,
                curve.pitch_offset_cents,
                curve.formant_offset_cents,
                anchors,
            ) {
                return false;
            }
        }
        writer.raw(b"]");
        return true;
    }

    let mut segment_count = 0usize;
    let mut anchor_total = 0usize;
    if !unsafe {
        hirari_region_vari_audio_store_sizes(region.note_store, &mut segment_count, &mut anchor_total)
    } {
        return false;
    }
    // The note store already enforces these bounds; they only keep the
    // materialization allocation honest if a foreign store is passed in.
    if segment_count > 4096 || anchor_total > 4096 * 4096 {
        return false;
    }
    if segment_count > 0 {
        let mut segments = vec![HirariAudioNoteSegment::default(); segment_count];
        let mut anchors = vec![HirariAudioNoteAnchor::default(); anchor_total];
        let mut offsets = vec![0usize; segment_count];
        let mut counts = vec![0usize; segment_count];
        let mut written_segments = 0usize;
        let mut written_anchors = 0usize;
        if !unsafe {
            hirari_region_vari_audio_store_materialize(
                region.note_store,
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
            return false;
        }
        for index in 0..written_segments {
            if writer.cancelled() {
                return false;
            }
            let segment = segments[index];
            let start = offsets[index];
            let count = counts[index];
            if start > written_anchors || count > written_anchors - start {
                return false;
            }
            if !first {
                writer.raw(b",");
            }
            first = false;
            if !write_note_segment(
                writer,
                segment.start_seconds,
                segment.end_seconds,
                segment.detected_pitch_cents,
                segment.pitch_offset_cents,
                segment.formant_offset_cents,
                &anchors[start..start + count],
            ) {
                return false;
            }
        }
    }
    writer.raw(b"]");
    true
}

unsafe fn write_region_processing_history(
    writer: &mut LayoutWriter,
    region: &HirariLayoutRegion,
) -> bool {
    if region.processing_store.is_null() {
        return false;
    }
    let mut count = 0usize;
    let mut byte_total = 0usize;
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_sizes(
            region.processing_store,
            &mut count,
            &mut byte_total,
        )
    } {
        return false;
    }
    if count > 4096 || byte_total > 1_048_576 {
        return false;
    }
    let mut views = vec![HirariEventStepView::default(); count];
    let mut bytes = vec![0u8; byte_total];
    let mut written_count = 0usize;
    let mut written_bytes = 0usize;
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_materialize(
            region.processing_store,
            views.as_mut_ptr(),
            views.len(),
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut written_count,
            &mut written_bytes,
        )
    } {
        return false;
    }
    writer.raw(b"[");
    for (index, view) in views.iter().take(written_count).enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        if view.operation_offset > written_bytes
            || view.operation_len > written_bytes - view.operation_offset
        {
            return false;
        }
        let operation = &bytes[view.operation_offset..view.operation_offset + view.operation_len];
        writer.raw(b"{\"id\":");
        writer.raw(view.id.to_string().as_bytes());
        writer.raw(b",\"operation\":\"");
        writer.text(operation);
        writer.raw(b"\",\"parameter\":");
        writer.number32(view.parameter);
        writer.raw(b",\"enabled\":");
        writer.boolean(view.enabled);
        writer.raw(b"}");
    }
    writer.raw(b"]");
    true
}

unsafe fn write_region(writer: &mut LayoutWriter, region: &HirariLayoutRegion) -> bool {
    let state = &region.state;
    let name = match unsafe { string_bytes(&region.name) } {
        Some(name) => name,
        None => return false,
    };
    let path = match unsafe { string_bytes(&region.path) } {
        Some(path) => path,
        None => return false,
    };
    let markers = match unsafe { optional_slice(region.warp_markers, region.warp_marker_count) } {
        Some(markers) => markers,
        None => return false,
    };
    let ranges = match unsafe { optional_slice(region.range_edits, region.range_edit_count) } {
        Some(ranges) => ranges,
        None => return false,
    };

    writer.raw(b"{\"id\":");
    writer.raw(region.id.to_string().as_bytes());
    writer.raw(b",\"name\":\"");
    writer.text(name);
    writer.raw(b"\",\"start\":");
    writer.raw(state.start.to_string().as_bytes());
    writer.raw(b",\"len\":");
    writer.raw(state.len.to_string().as_bytes());
    writer.raw(b",\"source_length\":");
    writer.raw(state.source_length.to_string().as_bytes());
    writer.raw(b",\"source_sample_rate\":");
    writer.raw(region.source_sample_rate.to_string().as_bytes());
    writer.raw(b",\"source_offset\":");
    writer.raw(state.source_offset.to_string().as_bytes());
    writer.raw(b",\"base_start\":");
    writer.raw(state.base_start.to_string().as_bytes());
    writer.raw(b",\"base_source_offset\":");
    writer.raw(state.base_source_offset.to_string().as_bytes());
    writer.raw(b",\"base_length\":");
    writer.raw(state.base_length.to_string().as_bytes());
    writer.raw(b",\"muted\":");
    writer.boolean(state.muted);
    writer.raw(b",\"path\":\"");
    writer.text(path);
    writer.raw(b"\",\"missing\":");
    writer.boolean(region.missing);
    writer.raw(b",\"audio_loaded\":");
    writer.boolean(region.audio_loaded);
    writer.raw(b",\"clip_gain\":");
    writer.number32(state.clip_gain);
    writer.raw(b",\"fade_in_samples\":");
    writer.raw(state.fade_in_samples.to_string().as_bytes());
    writer.raw(b",\"fade_out_samples\":");
    writer.raw(state.fade_out_samples.to_string().as_bytes());
    writer.raw(b",\"reverse\":");
    writer.boolean(state.reverse);
    writer.raw(b",\"warp_ratio\":");
    writer.number(state.warp_ratio);
    writer.raw(b",\"pitch_preserve_warp\":");
    writer.boolean(state.pitch_preserve_warp);
    writer.raw(b",\"pitch_semitones\":");
    writer.number32(state.pitch_semitones);
    writer.raw(b",\"loop_count\":");
    writer.raw(state.loop_count.to_string().as_bytes());
    writer.raw(b",\"sync_group\":");
    writer.raw(state.sync_group.to_string().as_bytes());
    writer.raw(b",\"warp_markers\":[");
    for (index, marker) in markers.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"source_sample\":");
        writer.raw(marker.source_sample.to_string().as_bytes());
        writer.raw(b",\"timeline_sample\":");
        writer.raw(marker.timeline_sample.to_string().as_bytes());
        writer.raw(b",\"transient\":");
        writer.boolean(marker.transient != 0);
        writer.raw(b"}");
    }
    writer.raw(b"],\"range_edits\":[");
    for (index, edit) in ranges.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"start\":");
        writer.raw(edit.start.to_string().as_bytes());
        writer.raw(b",\"end\":");
        writer.raw(edit.end.to_string().as_bytes());
        writer.raw(b",\"gain\":");
        writer.number32(edit.gain);
        writer.raw(b",\"fade_in\":");
        writer.raw(edit.fade_in.to_string().as_bytes());
        writer.raw(b",\"fade_out\":");
        writer.raw(edit.fade_out.to_string().as_bytes());
        writer.raw(b"}");
    }
    writer.raw(b"],\"audio_note_segments\":");
    if !unsafe { write_region_note_segments(writer, region) } {
        return false;
    }
    writer.raw(b",\"processing_history\":");
    if !unsafe { write_region_processing_history(writer, region) } {
        return false;
    }
    writer.raw(b"}");
    true
}

fn marker_value(parameters: &[HirariLayoutParameterBlock], index: usize) -> Option<f32> {
    let Some(block) = parameters.get(index) else {
        return Some(0.0);
    };
    let values = unsafe { optional_slice(block.values, block.count) }?;
    Some(values.first().copied().unwrap_or(0.0))
}

unsafe fn write_track(writer: &mut LayoutWriter, track: &HirariLayoutTrack) -> bool {
    if writer.cancelled() {
        return false;
    }
    let name = match unsafe { string_bytes(&track.name) } {
        Some(name) => name,
        None => return false,
    };
    let frozen_path = match unsafe { string_bytes(&track.frozen_path) } {
        Some(path) => path,
        None => return false,
    };
    let expression_map = match unsafe { string_bytes(&track.expression_map_json) } {
        Some(json) => json,
        None => return false,
    };
    let expression_map_pro = match unsafe { string_bytes(&track.expression_map_pro_json) } {
        Some(json) => json,
        None => return false,
    };
    let volume_automation =
        match unsafe { optional_slice(track.volume_automation, track.volume_automation_count) } {
            Some(points) => points,
            None => return false,
        };
    let pan_automation =
        match unsafe { optional_slice(track.pan_automation, track.pan_automation_count) } {
            Some(points) => points,
            None => return false,
        };
    let delay_automation = match unsafe {
        optional_slice(track.track_delay_automation, track.track_delay_automation_count)
    } {
        Some(points) => points,
        None => return false,
    };
    let lanes = match unsafe {
        optional_slice(track.plugin_automation, track.plugin_automation_count)
    } {
        Some(lanes) => lanes,
        None => return false,
    };
    let plugin_types = match unsafe { optional_slice(track.plugin_types, track.plugin_type_count) } {
        Some(types) => types,
        None => return false,
    };
    let bypass = match unsafe { optional_slice(track.plugin_bypass, plugin_types.len()) } {
        Some(bypass) => bypass,
        None => return false,
    };
    let parameters =
        match unsafe { optional_slice(track.plugin_parameters, plugin_types.len()) } {
            Some(parameters) => parameters,
            None => return false,
        };
    let states = match unsafe {
        optional_slice(track.plugin_states, track.plugin_state_count)
    } {
        Some(states) => states,
        None => return false,
    };
    let sandbox_paths = match unsafe {
        optional_slice(track.sandbox_plugin_paths, track.sandbox_plugin_path_count)
    } {
        Some(paths) => paths,
        None => return false,
    };
    let sandbox_states = match unsafe {
        optional_slice(track.sandbox_plugin_states, track.sandbox_plugin_state_count)
    } {
        Some(states) => states,
        None => return false,
    };
    let regions = match unsafe { optional_slice(track.regions, track.region_count) } {
        Some(regions) => regions,
        None => return false,
    };
    for state in states {
        if unsafe { optional_slice(state.bytes, state.count) }.is_none() {
            return false;
        }
    }
    for state in sandbox_states {
        if unsafe { optional_slice(state.bytes, state.count) }.is_none() {
            return false;
        }
    }
    for path in sandbox_paths {
        if unsafe { optional_slice(path.data, path.len) }.is_none() {
            return false;
        }
    }

    let type_name: &[u8] = match track.track_type {
        1 => b"Midi",
        2 => b"Instrument",
        3 => b"Bus",
        4 => b"Vocal",
        _ => b"Audio",
    };

    writer.raw(b"{\"id\":");
    writer.raw(track.id.to_string().as_bytes());
    writer.raw(b",\"name\":\"");
    writer.text(name);
    writer.raw(b"\",\"type\":\"");
    writer.raw(type_name);
    writer.raw(b"\",\"volume\":");
    writer.number32(track.volume);
    writer.raw(b",\"pan\":");
    writer.number32(track.pan);
    writer.raw(b",\"mute\":");
    writer.boolean(track.muted);
    writer.raw(b",\"solo\":");
    writer.boolean(track.solo);
    writer.raw(b",\"record_armed\":");
    writer.boolean(track.record_armed);
    writer.raw(b",\"phase_invert\":");
    writer.boolean(track.phase_inverted);
    writer.raw(b",\"track_delay_samples\":");
    writer.raw(track.track_delay_samples.to_string().as_bytes());
    writer.raw(b",\"frozen\":");
    writer.boolean(track.frozen);
    writer.raw(b",\"frozen_total_samples\":");
    writer.raw(track.frozen_total_samples.to_string().as_bytes());
    writer.raw(b",\"frozen_sample_rate\":");
    writer.raw(track.frozen_sample_rate.to_string().as_bytes());
    writer.raw(b",\"frozen_path\":\"");
    writer.text(frozen_path);
    writer.raw(b"\",\"volume_automation\":");
    if !write_automation_points(writer, volume_automation) {
        return false;
    }
    writer.raw(b",\"pan_automation\":");
    if !write_automation_points(writer, pan_automation) {
        return false;
    }
    writer.raw(b",\"track_delay_automation\":");
    if !write_automation_points(writer, delay_automation) {
        return false;
    }
    writer.raw(b",\"expression_map\":");
    writer.raw(expression_map);
    writer.raw(b",\"expression_map_pro\":");
    writer.raw(expression_map_pro);
    writer.raw(b",\"plugin_automation\":[");
    for (lane_index, lane) in lanes.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if lane_index > 0 {
            writer.raw(b",");
        }
        let points = match unsafe { optional_slice(lane.points, lane.point_count) } {
            Some(points) => points,
            None => return false,
        };
        writer.raw(b"{\"plugin_index\":");
        writer.raw(lane.plugin_index.to_string().as_bytes());
        writer.raw(b",\"parameter_id\":");
        writer.raw(lane.parameter_id.to_string().as_bytes());
        writer.raw(b",\"points\":[");
        for (point_index, point) in points.iter().enumerate() {
            if writer.cancelled() {
                return false;
            }
            if point_index > 0 {
                writer.raw(b",");
            }
            writer.raw(b"{\"sample\":");
            writer.raw((point.time as u64).to_string().as_bytes());
            writer.raw(b",\"normalized\":");
            writer.number32(point.value);
            writer.raw(b",\"curve\":");
            writer.number32(point.curve);
            writer.raw(b"}");
        }
        writer.raw(b"]}");
    }
    writer.raw(b"],\"plugin_types\":[");
    for (index, plugin_type) in plugin_types.iter().enumerate() {
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(plugin_type.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_bypass\":[");
    for (index, value) in bypass.iter().enumerate() {
        if index > 0 {
            writer.raw(b",");
        }
        writer.boolean(*value != 0);
    }
    writer.raw(b"],\"plugin_parameter_values\":[");
    for (plugin_index, block) in parameters.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if plugin_index > 0 {
            writer.raw(b",");
        }
        let values = match unsafe { optional_slice(block.values, block.count) } {
            Some(values) => values,
            None => return false,
        };
        writer.raw(b"[");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                writer.raw(b",");
            }
            writer.parameter(*value);
        }
        writer.raw(b"]");
    }
    writer.raw(b"],\"plugin_state_sizes\":[");
    let padded_state_count = states.len().max(plugin_types.len());
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let size = match states.get(index) {
            Some(state) => state.count,
            None => std::mem::size_of::<f32>(),
        };
        writer.raw(size.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_state_hashes\":[");
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let hash = match states.get(index) {
            Some(state) => {
                let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
                    Some(bytes) => bytes,
                    None => return false,
                };
                fnv1a(bytes)
            }
            None => match marker_value(parameters, index) {
                Some(value) => fnv1a(&value.to_ne_bytes()),
                None => return false,
            },
        };
        writer.raw(hash.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_state_hex\":[");
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"\"");
        match states.get(index) {
            Some(state) => {
                let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
                    Some(bytes) => bytes,
                    None => return false,
                };
                writer.hex(bytes);
            }
            None => match marker_value(parameters, index) {
                Some(value) => writer.hex(&value.to_ne_bytes()),
                None => return false,
            },
        }
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sandbox_plugin_paths\":[");
    for (index, path) in sandbox_paths.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"\"");
        let bytes = match unsafe { optional_slice(path.data, path.len) } {
            Some(bytes) => bytes,
            None => return false,
        };
        writer.text(bytes);
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sandbox_plugin_state_hex\":[");
    for (index, state) in sandbox_states.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
            Some(bytes) => bytes,
            None => return false,
        };
        writer.raw(b"\"");
        writer.hex(bytes);
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sidechain_routes\":[");
    let mut first_sidechain = true;
    let sidechain_routes = writer.sidechain_routes;
    for route in sidechain_routes
        .iter()
        .filter(|route| route.destination_id == track.id)
    {
        if writer.cancelled() {
            return false;
        }
        if !first_sidechain {
            writer.raw(b",");
        }
        first_sidechain = false;
        writer.raw(b"{\"source_id\":");
        writer.raw(route.source_id.to_string().as_bytes());
        writer.raw(b",\"destination_id\":");
        writer.raw(route.destination_id.to_string().as_bytes());
        writer.raw(b",\"plugin_index\":");
        writer.raw(route.plugin_index.to_string().as_bytes());
        writer.raw(b",\"tap_point\":");
        writer.raw(route.tap_point.to_string().as_bytes());
        writer.raw(b"}");
    }
    writer.raw(b"],\"feedback_routes\":[");
    let mut first_feedback = true;
    let feedback_routes = writer.feedback_routes;
    for route in feedback_routes
        .iter()
        .filter(|route| route.source_id == track.id)
    {
        if writer.cancelled() {
            return false;
        }
        if !first_feedback {
            writer.raw(b",");
        }
        first_feedback = false;
        writer.raw(b"{\"source_id\":");
        writer.raw(route.source_id.to_string().as_bytes());
        writer.raw(b",\"destination_id\":");
        writer.raw(route.destination_id.to_string().as_bytes());
        writer.raw(b",\"gain\":");
        writer.number32(route.gain);
        writer.raw(b"}");
    }
    writer.raw(b"],\"regions\":[");
    for (index, region) in regions.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        if !unsafe { write_region(writer, region) } {
            return false;
        }
    }
    writer.raw(b"]}");
    true
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

impl<'a> LayoutWriter<'a> {
    fn snapshot(&mut self, snapshot: &HirariProjectLayoutSnapshot) -> bool {
        let tracks = match unsafe { optional_slice(snapshot.tracks, snapshot.track_count) } {
            Some(tracks) => tracks,
            None => return false,
        };
        let routes =
            match unsafe { optional_slice(snapshot.sidechain_routes, snapshot.sidechain_route_count) }
            {
                Some(routes) => routes,
                None => return false,
            };
        let feedback =
            match unsafe { optional_slice(snapshot.feedback_routes, snapshot.feedback_route_count) }
            {
                Some(feedback) => feedback,
                None => return false,
            };
        self.sidechain_routes = routes;
        self.feedback_routes = feedback;
        self.raw(b"[");
        let mut serialized = 0usize;
        for track in tracks {
            if self.cancelled() {
                return false;
            }
            // The bootstrap track is a placeholder when a real project has
            // replaced it; it was never part of the saved layout.
            let is_empty_bootstrap = track.id == 0
                && unsafe { string_bytes(&track.name) } == Some(b"Audio 1".as_slice())
                && track.plugin_type_count == 0
                && track.region_count == 0
                && tracks.len() > 1;
            if is_empty_bootstrap {
                continue;
            }
            if serialized > 0 {
                self.raw(b",");
            }
            serialized += 1;
            if !unsafe { write_track(self, track) } {
                return false;
            }
        }
        self.raw(b"]");
        true
    }
}

/// Writes the whole project layout as JSON into a Rust-owned buffer.
///
/// # Safety
/// `snapshot` and its nested pointers must describe live data for the
/// duration of the call. `json_data`/`json_size` must be writable.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_write_json(
    snapshot: *const HirariProjectLayoutSnapshot,
    is_cancelled: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    cancel_context: *const c_void,
    json_data: *mut *mut u8,
    json_size: *mut usize,
) -> bool {
    if snapshot.is_null() || json_data.is_null() || json_size.is_null() {
        return false;
    }
    let mut writer = LayoutWriter {
        out: Vec::new(),
        cancel: is_cancelled.map(|callback| (callback, cancel_context)),
        sidechain_routes: &[],
        feedback_routes: &[],
    };
    if !writer.snapshot(unsafe { &*snapshot }) {
        return false;
    }
    let mut bytes = writer.out.into_boxed_slice();
    unsafe {
        *json_size = bytes.len();
        *json_data = bytes.as_mut_ptr();
    }
    std::mem::forget(bytes);
    true
}

/// # Safety
/// `json_data`/`json_size` must be the pair returned by
/// `hirari_project_layout_write_json`.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_json_free(json_data: *const u8, json_size: usize) {
    if json_data.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(json_data.cast_mut(), json_size);
    drop(unsafe { Box::from_raw(slice) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region_processing_history::HirariEventStep;

    #[test]
    fn escapes_exactly_the_bytes_the_cpp_writer_escaped() {
        let mut writer = LayoutWriter {
            out: Vec::new(),
            cancel: None,
            sidechain_routes: &[],
            feedback_routes: &[],
        };
        writer.text(b"a\"b\\c\nd\re\tf\x01g");
        assert_eq!(writer.out, b"a\\\"b\\\\c\\nd\\re\\tf?g");
    }

    fn layout_string(value: &str) -> HirariLayoutString {
        HirariLayoutString {
            data: value.as_ptr(),
            len: value.len(),
        }
    }

    fn write_json(snapshot: &HirariProjectLayoutSnapshot) -> Option<String> {
        let mut data = std::ptr::null_mut();
        let mut size = 0usize;
        let ok = unsafe {
            hirari_project_layout_write_json(
                snapshot,
                None,
                std::ptr::null(),
                &mut data,
                &mut size,
            )
        };
        if !ok {
            assert!(data.is_null());
            return None;
        }
        let text = String::from_utf8(unsafe { std::slice::from_raw_parts(data, size) }.to_vec())
            .expect("layout JSON is UTF-8");
        unsafe { hirari_project_layout_json_free(data, size) };
        Some(text)
    }

    fn empty_track(id: u32, name: &str) -> HirariLayoutTrack {
        HirariLayoutTrack {
            id,
            name: layout_string(name),
            volume: 1.0,
            frozen_sample_rate: 44_100,
            ..HirariLayoutTrack::default()
        }
    }

    struct Stores {
        notes: *mut HirariRegionVariAudioStore,
        history: *mut HirariRegionProcessingHistoryStore,
        frozen: *mut HirariVariAudioFrozen,
    }

    impl Stores {
        fn new() -> Self {
            Stores {
                notes: unsafe { crate::vari_audio::hirari_region_vari_audio_store_new() },
                history: unsafe {
                    crate::region_processing_history::hirari_region_processing_history_new()
                },
                frozen: std::ptr::null_mut(),
            }
        }
    }

    impl Drop for Stores {
        fn drop(&mut self) {
            unsafe {
                if !self.frozen.is_null() {
                    crate::vari_audio::hirari_region_vari_audio_frozen_free(self.frozen);
                }
                crate::vari_audio::hirari_region_vari_audio_store_clear(self.notes);
                crate::region_processing_history::hirari_region_processing_history_clear(
                    self.history,
                );
            }
        }
    }

    #[test]
    fn serializes_the_full_layout_with_the_legacy_key_order_and_number_format() {
        let stores = Stores::new();
        let segment = HirariAudioNoteSegment {
            start_seconds: 1.25,
            end_seconds: 2.5,
            detected_pitch_cents: 6900.0,
            pitch_offset_cents: 10.0,
            formant_offset_cents: -20.0,
        };
        let anchor = HirariAudioNoteAnchor {
            position_seconds: 1.5,
            pitch_cents: 6900.0,
            formant_cents: 0.0,
        };
        assert!(unsafe {
            crate::vari_audio::hirari_region_vari_audio_store_upsert(
                stores.notes,
                segment,
                &anchor,
                1,
            )
        });
        let operation = b"gain";
        let step = HirariEventStep {
            id: 1,
            operation: operation.as_ptr(),
            operation_len: operation.len(),
            parameter: 0.5,
            enabled: true,
        };
        assert!(unsafe {
            crate::region_processing_history::hirari_region_processing_history_append(
                stores.history,
                step,
            )
        });

        let track_name = "Lead \"A\"";
        let expression_map = "{}";
        let volume_automation = [HirariLayoutAutomationPoint {
            time: 0.5,
            value: 0.25,
            curve: 0.0,
        }];
        let plugin_types = [1u32, 2];
        let plugin_bypass = [0u8, 1];
        let first_parameters = [0.5f32, 1.0 / 3.0];
        let second_parameters: [f32; 0] = [];
        let plugin_parameters = [
            HirariLayoutParameterBlock {
                values: first_parameters.as_ptr(),
                count: first_parameters.len(),
            },
            HirariLayoutParameterBlock {
                values: second_parameters.as_ptr(),
                count: 0,
            },
        ];
        let first_state = [0x00u8, 0xab];
        let plugin_states = [HirariLayoutByteBlock {
            bytes: first_state.as_ptr(),
            count: first_state.len(),
        }];
        let sidechain_routes = [HirariLayoutSidechainRoute {
            source_id: 9,
            destination_id: 5,
            plugin_index: 1,
            tap_point: 2,
        }];
        let feedback_routes = [HirariLayoutFeedbackRoute {
            source_id: 5,
            destination_id: 7,
            gain: 0.25,
        }];

        let region_name = "Vox";
        let region_path = "/tmp/missing.wav";
        let warp_markers = [HirariWarpMarker {
            source_sample: 0,
            timeline_sample: 0,
            transient: 1,
        }];
        let range_edits = [NativeRangeEdit {
            start: 0,
            end: 10,
            gain: 0.5,
            fade_in: 1,
            fade_out: 2,
        }];
        let regions = [HirariLayoutRegion {
            id: 100,
            name: layout_string(region_name),
            state: HirariRegionState {
                start: 1000,
                len: 2000,
                base_start: 1000,
                base_length: 2000,
                source_offset: 50,
                base_source_offset: 50,
                source_length: 2200,
                fade_in_samples: 10,
                fade_out_samples: 20,
                warp_ratio: 1.0,
                source_sample_rate: 48_000.0,
                timeline_sample_rate: 48_000.0,
                clip_gain: 0.75,
                pitch_semitones: 0.0,
                loop_count: 1,
                sync_group: 3,
                muted: false,
                reverse: false,
                locked: false,
                pitch_preserve_warp: false,
            },
            source_sample_rate: 48_000,
            path: layout_string(region_path),
            missing: true,
            audio_loaded: false,
            warp_markers: warp_markers.as_ptr(),
            warp_marker_count: warp_markers.len(),
            range_edits: range_edits.as_ptr(),
            range_edit_count: range_edits.len(),
            note_store: stores.notes,
            note_frozen: std::ptr::null(),
            processing_store: stores.history,
        }];

        let tracks = [HirariLayoutTrack {
            id: 5,
            name: layout_string(track_name),
            track_type: 1,
            volume: 0.5,
            pan: -0.25,
            muted: false,
            solo: true,
            record_armed: false,
            phase_inverted: true,
            track_delay_samples: 128,
            frozen: false,
            frozen_total_samples: 0,
            frozen_sample_rate: 44_100,
            frozen_path: layout_string(""),
            volume_automation: volume_automation.as_ptr(),
            volume_automation_count: volume_automation.len(),
            pan_automation: std::ptr::null(),
            pan_automation_count: 0,
            track_delay_automation: std::ptr::null(),
            track_delay_automation_count: 0,
            expression_map_json: layout_string(expression_map),
            expression_map_pro_json: layout_string(expression_map),
            plugin_automation: std::ptr::null(),
            plugin_automation_count: 0,
            plugin_types: plugin_types.as_ptr(),
            plugin_type_count: plugin_types.len(),
            plugin_bypass: plugin_bypass.as_ptr(),
            plugin_parameters: plugin_parameters.as_ptr(),
            plugin_states: plugin_states.as_ptr(),
            plugin_state_count: plugin_states.len(),
            sandbox_plugin_paths: std::ptr::null(),
            sandbox_plugin_path_count: 0,
            sandbox_plugin_states: std::ptr::null(),
            sandbox_plugin_state_count: 0,
            regions: regions.as_ptr(),
            region_count: regions.len(),
        }];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            sidechain_routes: sidechain_routes.as_ptr(),
            sidechain_route_count: sidechain_routes.len(),
            feedback_routes: feedback_routes.as_ptr(),
            feedback_route_count: feedback_routes.len(),
            timeline_sample_rate: 48_000.0,
        };

        let json = write_json(&snapshot).expect("layout serializes");

        let marker_hash = fnv1a(&0.0f32.to_ne_bytes()).to_string();
        let region = concat!(
            r#"{"id":100,"name":"Vox","start":1000,"len":2000,"source_length":2200,"#,
            r#""source_sample_rate":48000,"source_offset":50,"base_start":1000,"#,
            r#""base_source_offset":50,"base_length":2000,"muted":false,"#,
            r#""path":"/tmp/missing.wav","missing":true,"audio_loaded":false,"#,
            r#""clip_gain":0.750000,"fade_in_samples":10,"fade_out_samples":20,"#,
            r#""reverse":false,"warp_ratio":1.000000,"pitch_preserve_warp":false,"#,
            r#""pitch_semitones":0.000000,"loop_count":1,"sync_group":3,"#,
            r#""warp_markers":[{"source_sample":0,"timeline_sample":0,"transient":true}],"#,
            r#""range_edits":[{"start":0,"end":10,"gain":0.500000,"fade_in":1,"fade_out":2}],"#,
            r#""audio_note_segments":[{"start":1.250000,"end":2.500000,"#,
            r#""detected_pitch_cents":6900.000000,"pitch_offset_cents":10.000000,"#,
            r#""formant_offset_cents":-20.000000,"anchors":[{"position":1.500000,"#,
            r#""pitch_cents":6900.000000,"formant_cents":0.000000}]}],"#,
            r#""processing_history":[{"id":1,"operation":"gain","parameter":0.500000,"enabled":true}]}"#,
        );
        let expected = format!(
            concat!(
                r#"[{{"id":5,"name":"Lead \"A\"","type":"Midi","volume":0.500000,"#,
                r#""pan":-0.250000,"mute":false,"solo":true,"record_armed":false,"#,
                r#""phase_invert":true,"track_delay_samples":128,"frozen":false,"#,
                r#""frozen_total_samples":0,"frozen_sample_rate":44100,"frozen_path":"","#,
                r#""volume_automation":[{{"time":0.500000,"value":0.250000,"curve":0.000000}}],"#,
                r#""pan_automation":[],"track_delay_automation":[],"#,
                r#""expression_map":{{}},"expression_map_pro":{{}},"plugin_automation":[],"#,
                r#""plugin_types":[1,2],"plugin_bypass":[false,true],"#,
                r#""plugin_parameter_values":[[0.500000,0.333333],[]],"#,
                r#""plugin_state_sizes":[2,4],"plugin_state_hashes":[{},{}],"#,
                r#""plugin_state_hex":["00ab","00000000"],"sandbox_plugin_paths":[],"#,
                r#""sandbox_plugin_state_hex":[],"#,
                r#""sidechain_routes":[{{"source_id":9,"destination_id":5,"#,
                r#""plugin_index":1,"tap_point":2}}],"#,
                r#""feedback_routes":[{{"source_id":5,"destination_id":7,"gain":0.250000}}],"#,
                r#""regions":[{}]}}]"#,
            ),
            fnv1a(&first_state),
            marker_hash,
            region,
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn skips_the_empty_bootstrap_track() {
        let bootstrap_name = "Audio 1";
        let other_name = "Audio 2";
        let tracks = [
            empty_track(0, bootstrap_name),
            empty_track(1, other_name),
        ];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.starts_with(r#"[{"id":1,"name":"Audio 2","#));
        assert!(!json.contains("Audio 1"));
    }

    #[test]
    fn keeps_the_bootstrap_track_when_it_is_the_only_track() {
        let bootstrap_name = "Audio 1";
        let tracks = [empty_track(0, bootstrap_name)];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.starts_with(r#"[{"id":0,"name":"Audio 1","#));
    }

    #[test]
    fn cancellation_aborts_the_layout() {
        let other_name = "Audio 2";
        let tracks = [empty_track(1, other_name)];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        unsafe extern "C" fn cancel(_: *const c_void) -> bool {
            true
        }
        let mut data = std::ptr::null_mut();
        let mut size = 0usize;
        assert!(!unsafe {
            hirari_project_layout_write_json(
                &snapshot,
                Some(cancel),
                std::ptr::null(),
                &mut data,
                &mut size,
            )
        });
        assert!(data.is_null());
    }

    #[test]
    fn non_finite_numbers_fall_back_to_the_legacy_placeholders() {
        let other_name = "Audio 2";
        let plugin_types = [1u32];
        let plugin_bypass = [0u8];
        let parameters = [f32::NAN];
        let blocks = [HirariLayoutParameterBlock {
            values: parameters.as_ptr(),
            count: parameters.len(),
        }];
        let tracks = [HirariLayoutTrack {
            volume: f32::NAN,
            plugin_types: plugin_types.as_ptr(),
            plugin_type_count: plugin_types.len(),
            plugin_bypass: plugin_bypass.as_ptr(),
            plugin_parameters: blocks.as_ptr(),
            ..empty_track(1, other_name)
        }];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.contains(r#""volume":0,"#));
        assert!(json.contains(r#""plugin_parameter_values":[[0.0]]"#));
    }

    #[test]
    fn published_note_projection_matches_the_store_materialization() {
        let stores = Stores::new();
        let segment = HirariAudioNoteSegment {
            start_seconds: 1.0,
            end_seconds: 2.0,
            detected_pitch_cents: 4400.0,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        assert!(unsafe {
            crate::vari_audio::hirari_region_vari_audio_store_upsert(
                stores.notes,
                segment,
                std::ptr::null(),
                0,
            )
        });
        let frozen = unsafe { crate::vari_audio::hirari_region_vari_audio_freeze(stores.notes) };
        assert!(!frozen.is_null());

        let region_name = "Vox";
        let region_path = "";
        let mut region = HirariLayoutRegion {
            id: 100,
            name: layout_string(region_name),
            path: layout_string(region_path),
            source_sample_rate: 48_000,
            note_store: stores.notes,
            note_frozen: std::ptr::null(),
            processing_store: stores.history,
            ..HirariLayoutRegion::default()
        };
        let other_name = "Audio 2";
        let mut tracks = [HirariLayoutTrack {
            regions: &region,
            region_count: 1,
            ..empty_track(1, other_name)
        }];
        let from_store = write_json(&HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        })
        .expect("layout serializes");
        region.note_frozen = frozen;
        tracks[0].regions = &region;
        let from_frozen = write_json(&HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        })
        .expect("layout serializes");
        assert_eq!(from_store, from_frozen);
        assert!(from_frozen.contains(
            r#""audio_note_segments":[{"start":1.000000,"end":2.000000,"#,
        ));
    }
}
