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
    // Save-only fields. The JSON writer ignores them, so the layout gather
    // only fills them when a project save asked for the snapshot.
    pub spatial_x: f32,
    pub spatial_y: f32,
    pub spatial_z: f32,
    pub plugin_gui_states: *const HirariLayoutByteBlock,
    pub plugin_gui_state_count: usize,
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
#[path = "project_layout_writer.rs"]
mod writer;
#[path = "project_layout_track.rs"]
mod track;
pub(crate) use writer::*;

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
#[path = "project_layout_tests.rs"]
mod tests;
