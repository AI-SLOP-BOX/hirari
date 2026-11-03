//! Project-layout gathering.
//!
//! C++ owns the live tracks, so it exposes them through this ops table; every
//! snapshot rule — string ownership, automation copying, plug-in ordering,
//! region rate clamping, missing-file detection — lives here. The result owns
//! all borrowed blocks and stays valid until the JSON writer or the project
//! saver has consumed it.

use crate::project_layout::{
    hirari_project_layout_write_json, HirariLayoutAutomationPoint, HirariLayoutByteBlock,
    HirariLayoutFeedbackRoute, HirariLayoutParameterBlock, HirariLayoutPluginLane,
    HirariLayoutRegion, HirariLayoutSidechainRoute, HirariLayoutString, HirariLayoutTrack,
    HirariProjectLayoutSnapshot,
};
use crate::region_edit_host::HirariRegionState;
use crate::region_processing::NativeRangeEdit;
use crate::region_processing_history::HirariRegionProcessingHistoryStore;
use crate::region_warp::HirariWarpMarker;
use crate::vari_audio::{HirariRegionVariAudioStore, HirariVariAudioFrozen};
use std::ffi::c_void;

/// Track fields that have no nested storage. Copied out in one call.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutTrackScalars {
    pub id: u32,
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
    pub spatial_x: f32,
    pub spatial_y: f32,
    pub spatial_z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariLayoutPluginLaneHeader {
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub point_count: usize,
}

/// Region fields that have no owned strings. The borrowed pointers already
/// refer to the region's own storage, exactly as before.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariLayoutRegionHeader {
    pub id: u32,
    pub state: HirariRegionState,
    pub source_sample_rate: f64,
    pub audio_loaded: bool,
    pub warp_markers: *const HirariWarpMarker,
    pub warp_marker_count: usize,
    pub range_edits: *const NativeRangeEdit,
    pub range_edit_count: usize,
    pub note_store: *const HirariRegionVariAudioStore,
    pub note_frozen: *const HirariVariAudioFrozen,
    pub processing_store: *const HirariRegionProcessingHistoryStore,
}

impl Default for HirariLayoutRegionHeader {
    fn default() -> Self {
        Self {
            id: 0,
            state: HirariRegionState::default(),
            source_sample_rate: 0.0,
            audio_loaded: false,
            warp_markers: std::ptr::null(),
            warp_marker_count: 0,
            range_edits: std::ptr::null(),
            range_edit_count: 0,
            note_store: std::ptr::null(),
            note_frozen: std::ptr::null(),
            processing_store: std::ptr::null(),
        }
    }
}

type CopyBytes = unsafe extern "C" fn(*mut c_void, usize, u32, *mut u8, usize) -> usize;
type CopyPoints =
    unsafe extern "C" fn(*mut c_void, usize, u32, *mut HirariLayoutAutomationPoint, usize) -> usize;

/// Host accessors. `begin` flushes recorded automation and takes the tracks
/// lock so the borrowed region pointers stay stable for the whole gather;
/// `end` always runs, including on cancellation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariLayoutHostOps {
    pub begin: unsafe extern "C" fn(*mut c_void),
    pub end: unsafe extern "C" fn(*mut c_void),
    pub track_count: unsafe extern "C" fn(*mut c_void) -> usize,
    pub track_at:
        unsafe extern "C" fn(*mut c_void, usize, *mut HirariLayoutTrackScalars) -> bool,
    pub track_string: CopyBytes,
    pub automation: CopyPoints,
    pub plugin_lane_count: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    pub plugin_lane_at:
        unsafe extern "C" fn(*mut c_void, usize, usize, *mut HirariLayoutPluginLaneHeader) -> bool,
    pub plugin_lane_points: unsafe extern "C" fn(
        *mut c_void,
        usize,
        usize,
        *mut HirariLayoutAutomationPoint,
        usize,
    ) -> usize,
    pub plugin_count: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    pub plugin_type: unsafe extern "C" fn(*mut c_void, usize, usize) -> u32,
    pub plugin_bypass: unsafe extern "C" fn(*mut c_void, usize, usize) -> bool,
    pub plugin_state:
        unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8, usize) -> usize,
    pub plugin_gui_state:
        unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8, usize) -> usize,
    pub plugin_parameter_count: unsafe extern "C" fn(*mut c_void, usize, usize) -> usize,
    pub plugin_parameters:
        unsafe extern "C" fn(*mut c_void, usize, usize, *mut f32, usize) -> usize,
    pub sandbox_count: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    pub sandbox_path: unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8, usize) -> usize,
    pub sandbox_state: unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8, usize) -> usize,
    pub region_count: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    pub region_at: unsafe extern "C" fn(*mut c_void, usize, usize, *mut HirariLayoutRegionHeader)
        -> bool,
    pub region_string:
        unsafe extern "C" fn(*mut c_void, usize, usize, u32, *mut u8, usize) -> usize,
    /// The sidechain route table and the routing graph are Rust stores the
    /// engine only holds; Rust reads them through its own store API.
    pub sidechain_routes_store: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub routing_store: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub sample_rate: unsafe extern "C" fn(*mut c_void) -> f64,
}

struct LaneScratch {
    plugin_index: u32,
    parameter_id: u32,
    points: Vec<HirariLayoutAutomationPoint>,
}

struct TrackScratch {
    scalars: HirariLayoutTrackScalars,
    name: Vec<u8>,
    frozen_path: Vec<u8>,
    expression_map: Vec<u8>,
    expression_map_pro: Vec<u8>,
    volume_automation: Vec<HirariLayoutAutomationPoint>,
    pan_automation: Vec<HirariLayoutAutomationPoint>,
    track_delay_automation: Vec<HirariLayoutAutomationPoint>,
    lane_storage: Vec<LaneScratch>,
    lanes: Vec<HirariLayoutPluginLane>,
    plugin_types: Vec<u32>,
    plugin_bypass: Vec<u8>,
    plugin_parameters: Vec<Vec<f32>>,
    plugin_parameter_blocks: Vec<HirariLayoutParameterBlock>,
    plugin_states: Vec<Vec<u8>>,
    plugin_state_blocks: Vec<HirariLayoutByteBlock>,
    plugin_gui_states: Vec<Vec<u8>>,
    plugin_gui_state_blocks: Vec<HirariLayoutByteBlock>,
    sandbox_paths: Vec<Vec<u8>>,
    sandbox_path_views: Vec<HirariLayoutString>,
    sandbox_states: Vec<Vec<u8>>,
    sandbox_state_blocks: Vec<HirariLayoutByteBlock>,
    region_strings: Vec<Vec<u8>>,
    regions: Vec<HirariLayoutRegion>,
    view: HirariLayoutTrack,
}

impl TrackScratch {
    // Pointers are taken only once every backing block is final; no block is
    // pushed after this pass.
    fn link(&mut self, save_only: bool) {
        self.lanes = self
            .lane_storage
            .iter()
            .map(|lane| HirariLayoutPluginLane {
                plugin_index: lane.plugin_index,
                parameter_id: lane.parameter_id,
                points: lane.points.as_ptr(),
                point_count: lane.points.len(),
            })
            .collect();
        self.plugin_parameter_blocks = self
            .plugin_parameters
            .iter()
            .map(|values| HirariLayoutParameterBlock {
                values: values.as_ptr(),
                count: values.len(),
            })
            .collect();
        self.plugin_state_blocks = byte_blocks(&self.plugin_states);
        self.plugin_gui_state_blocks = byte_blocks(&self.plugin_gui_states);
        self.sandbox_path_views = self
            .sandbox_paths
            .iter()
            .map(|path| layout_string(path))
            .collect();
        self.sandbox_state_blocks = byte_blocks(&self.sandbox_states);
        for (index, region) in self.regions.iter_mut().enumerate() {
            region.name = layout_string(&self.region_strings[index * 2]);
            region.path = layout_string(&self.region_strings[index * 2 + 1]);
        }

        let scalars = self.scalars;
        self.view = HirariLayoutTrack {
            id: scalars.id,
            name: layout_string(&self.name),
            track_type: scalars.track_type,
            volume: scalars.volume,
            pan: scalars.pan,
            muted: scalars.muted,
            solo: scalars.solo,
            record_armed: scalars.record_armed,
            phase_inverted: scalars.phase_inverted,
            track_delay_samples: scalars.track_delay_samples,
            frozen: scalars.frozen,
            frozen_total_samples: scalars.frozen_total_samples,
            frozen_sample_rate: scalars.frozen_sample_rate,
            frozen_path: layout_string(&self.frozen_path),
            volume_automation: self.volume_automation.as_ptr(),
            volume_automation_count: self.volume_automation.len(),
            pan_automation: self.pan_automation.as_ptr(),
            pan_automation_count: self.pan_automation.len(),
            track_delay_automation: self.track_delay_automation.as_ptr(),
            track_delay_automation_count: self.track_delay_automation.len(),
            expression_map_json: layout_string(&self.expression_map),
            expression_map_pro_json: layout_string(&self.expression_map_pro),
            plugin_automation: self.lanes.as_ptr(),
            plugin_automation_count: self.lanes.len(),
            plugin_types: self.plugin_types.as_ptr(),
            plugin_type_count: self.plugin_types.len(),
            plugin_bypass: self.plugin_bypass.as_ptr(),
            // The JSON writer ignores both save-only blocks; only the saver
            // path fills GUI blobs, and only the polling path fills params.
            plugin_parameters: if save_only {
                std::ptr::null()
            } else {
                self.plugin_parameter_blocks.as_ptr()
            },
            plugin_states: self.plugin_state_blocks.as_ptr(),
            plugin_state_count: self.plugin_state_blocks.len(),
            sandbox_plugin_paths: self.sandbox_path_views.as_ptr(),
            sandbox_plugin_path_count: self.sandbox_path_views.len(),
            sandbox_plugin_states: self.sandbox_state_blocks.as_ptr(),
            sandbox_plugin_state_count: self.sandbox_state_blocks.len(),
            regions: self.regions.as_ptr(),
            region_count: self.regions.len(),
            spatial_x: scalars.spatial_x,
            spatial_y: scalars.spatial_y,
            spatial_z: scalars.spatial_z,
            plugin_gui_states: if save_only {
                self.plugin_gui_state_blocks.as_ptr()
            } else {
                std::ptr::null()
            },
            plugin_gui_state_count: if save_only {
                self.plugin_gui_state_blocks.len()
            } else {
                0
            },
        };
    }
}

fn layout_string(bytes: &[u8]) -> HirariLayoutString {
    HirariLayoutString {
        data: bytes.as_ptr(),
        len: bytes.len(),
    }
}

fn byte_blocks(blocks: &[Vec<u8>]) -> Vec<HirariLayoutByteBlock> {
    blocks
        .iter()
        .map(|bytes| HirariLayoutByteBlock {
            bytes: bytes.as_ptr(),
            count: bytes.len(),
        })
        .collect()
}

/// Copies the engine's sidechain route table out of its Rust store.
unsafe fn sidechain_route_list(store: *mut c_void) -> Vec<HirariLayoutSidechainRoute> {
    let count = unsafe { crate::sidechain_route_store::hirari_sidechain_routes_count(store) };
    let mut routes =
        vec![crate::sidechain_route_store::HirariSidechainRoute::default(); count];
    unsafe {
        crate::sidechain_route_store::hirari_sidechain_routes_copy(
            store,
            routes.as_mut_ptr(),
            count,
        )
    };
    routes
        .iter()
        .map(|route| HirariLayoutSidechainRoute {
            source_id: route.source_id,
            destination_id: route.destination_id,
            plugin_index: route.plugin_index,
            tap_point: route.tap_point,
        })
        .collect()
}

/// Copies the routing graph's feedback edges out of their Rust store.
unsafe fn feedback_route_list(store: *mut c_void) -> Vec<HirariLayoutFeedbackRoute> {
    let mut count = 0usize;
    let raw =
        unsafe { crate::routing_engine_core::hirari_routing_engine_copy_feedback(store, &mut count) };
    if raw.is_null() || count == 0 {
        return Vec::new();
    }
    let routes = unsafe { std::slice::from_raw_parts(raw, count) }
        .iter()
        .map(|edge| HirariLayoutFeedbackRoute {
            source_id: edge.source_id,
            destination_id: edge.destination_id,
            gain: edge.gain,
        })
        .collect();
    unsafe { crate::routing_engine_core::hirari_routing_feedback_array_free(raw, count) };
    routes
}

struct LayoutOwned {
    tracks: Box<[HirariLayoutTrack]>,
    sidechain_routes: Vec<HirariLayoutSidechainRoute>,
    feedback_routes: Vec<HirariLayoutFeedbackRoute>,
    timeline_sample_rate: f64,
    // Keeps every borrowed block the tracks point at alive.
    _scratch: Vec<TrackScratch>,
}

impl LayoutOwned {
    fn snapshot(&self) -> HirariProjectLayoutSnapshot {
        HirariProjectLayoutSnapshot {
            tracks: self.tracks.as_ptr(),
            track_count: self.tracks.len(),
            sidechain_routes: self.sidechain_routes.as_ptr(),
            sidechain_route_count: self.sidechain_routes.len(),
            feedback_routes: self.feedback_routes.as_ptr(),
            feedback_route_count: self.feedback_routes.len(),
            timeline_sample_rate: self.timeline_sample_rate,
        }
    }
}

unsafe fn copy_bytes(
    copy: unsafe extern "C" fn(*mut c_void, usize, u32, *mut u8, usize) -> usize,
    host: *mut c_void,
    index: usize,
    kind: u32,
) -> Vec<u8> {
    let length = unsafe { copy(host, index, kind, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe { copy(host, index, kind, data.as_mut_ptr(), length) };
    data.truncate(written.min(length));
    data
}

unsafe fn copy_region_bytes(
    copy: unsafe extern "C" fn(*mut c_void, usize, usize, u32, *mut u8, usize) -> usize,
    host: *mut c_void,
    index: usize,
    region: usize,
    kind: u32,
) -> Vec<u8> {
    let length = unsafe { copy(host, index, region, kind, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe { copy(host, index, region, kind, data.as_mut_ptr(), length) };
    data.truncate(written.min(length));
    data
}

unsafe fn copy_points(
    copy: unsafe extern "C" fn(
        *mut c_void,
        usize,
        u32,
        *mut HirariLayoutAutomationPoint,
        usize,
    ) -> usize,
    host: *mut c_void,
    index: usize,
    kind: u32,
) -> Vec<HirariLayoutAutomationPoint> {
    let count = unsafe { copy(host, index, kind, std::ptr::null_mut(), 0) };
    if count == 0 {
        return Vec::new();
    }
    let mut points = vec![HirariLayoutAutomationPoint::default(); count];
    let written = unsafe { copy(host, index, kind, points.as_mut_ptr(), count) };
    points.truncate(written.min(count));
    points
}

fn clamped_source_rate(raw: f64, timeline: f64) -> u32 {
    let value = if raw > 0.0 { raw } else { timeline };
    let rounded = if value.is_finite() { value.round() as i64 } else { 0 };
    rounded.clamp(8_000, 384_000) as u32
}

unsafe fn gather_track(
    host: *mut c_void,
    ops: &HirariLayoutHostOps,
    index: usize,
    save_only: bool,
    timeline_sample_rate: f64,
) -> Option<TrackScratch> {
    let copy = |kind: u32| unsafe { copy_bytes(ops.track_string, host, index, kind) };
    let mut scratch = TrackScratch {
        scalars: HirariLayoutTrackScalars::default(),
        name: copy(0),
        frozen_path: copy(1),
        expression_map: copy(2),
        expression_map_pro: copy(3),
        volume_automation: unsafe { copy_points(ops.automation, host, index, 0) },
        pan_automation: unsafe { copy_points(ops.automation, host, index, 1) },
        track_delay_automation: unsafe { copy_points(ops.automation, host, index, 2) },
        lane_storage: Vec::new(),
        lanes: Vec::new(),
        plugin_types: Vec::new(),
        plugin_bypass: Vec::new(),
        plugin_parameters: Vec::new(),
        plugin_parameter_blocks: Vec::new(),
        plugin_states: Vec::new(),
        plugin_state_blocks: Vec::new(),
        plugin_gui_states: Vec::new(),
        plugin_gui_state_blocks: Vec::new(),
        sandbox_paths: Vec::new(),
        sandbox_path_views: Vec::new(),
        sandbox_states: Vec::new(),
        sandbox_state_blocks: Vec::new(),
        region_strings: Vec::new(),
        regions: Vec::new(),
        view: HirariLayoutTrack::default(),
    };
    if !unsafe { (ops.track_at)(host, index, &mut scratch.scalars) } {
        return None;
    }

    let lane_count = unsafe { (ops.plugin_lane_count)(host, index) };
    scratch.lane_storage.reserve(lane_count);
    for lane in 0..lane_count {
        let mut header = HirariLayoutPluginLaneHeader::default();
        if !unsafe { (ops.plugin_lane_at)(host, index, lane, &mut header) } {
            continue;
        }
        let mut points = vec![HirariLayoutAutomationPoint::default(); header.point_count];
        if header.point_count > 0 {
            let written = unsafe {
                (ops.plugin_lane_points)(host, index, lane, points.as_mut_ptr(), points.len())
            };
            points.truncate(written.min(points.len()));
        }
        scratch.lane_storage.push(LaneScratch {
            plugin_index: header.plugin_index,
            parameter_id: header.parameter_id,
            points,
        });
    }

    let plugin_count = unsafe { (ops.plugin_count)(host, index) };
    scratch.plugin_types.reserve(plugin_count);
    scratch.plugin_bypass.reserve(plugin_count);
    for plugin in 0..plugin_count {
        scratch
            .plugin_types
            .push(unsafe { (ops.plugin_type)(host, index, plugin) });
        scratch.plugin_bypass.push(
            if unsafe { (ops.plugin_bypass)(host, index, plugin) } {
                1
            } else {
                0
            },
        );
        let state = unsafe { copy_plugin_bytes(ops.plugin_state, host, index, plugin) };
        scratch.plugin_states.push(state);
        if save_only {
            let gui = unsafe { copy_plugin_bytes(ops.plugin_gui_state, host, index, plugin) };
            scratch.plugin_gui_states.push(gui);
        } else {
            let count = unsafe { (ops.plugin_parameter_count)(host, index, plugin) };
            let mut values = vec![0.0f32; count];
            if count > 0 {
                let written = unsafe {
                    (ops.plugin_parameters)(host, index, plugin, values.as_mut_ptr(), count)
                };
                values.truncate(written.min(count));
            }
            scratch.plugin_parameters.push(values);
        }
    }

    let sandbox_count = unsafe { (ops.sandbox_count)(host, index) };
    for slot in 0..sandbox_count {
        let path = unsafe { copy_plugin_bytes(ops.sandbox_path, host, index, slot) };
        scratch.sandbox_paths.push(path);
        let state = unsafe { copy_plugin_bytes(ops.sandbox_state, host, index, slot) };
        scratch.sandbox_states.push(state);
    }

    let region_count = unsafe { (ops.region_count)(host, index) };
    scratch.regions.reserve(region_count);
    scratch.region_strings.reserve(region_count * 2);
    for region_index in 0..region_count {
        let mut header = HirariLayoutRegionHeader::default();
        if !unsafe { (ops.region_at)(host, index, region_index, &mut header) } {
            continue;
        }
        let name = unsafe { copy_region_bytes(ops.region_string, host, index, region_index, 0) };
        let path = unsafe { copy_region_bytes(ops.region_string, host, index, region_index, 1) };
        let missing = path.is_empty() || std::fs::metadata(path_bytes(&path)).is_err();
        scratch.region_strings.push(name);
        scratch.region_strings.push(path);
        scratch.regions.push(HirariLayoutRegion {
            id: header.id,
            name: HirariLayoutString::default(),
            state: header.state,
            source_sample_rate: clamped_source_rate(
                header.source_sample_rate,
                timeline_sample_rate,
            ),
            path: HirariLayoutString::default(),
            missing,
            audio_loaded: header.audio_loaded,
            warp_markers: header.warp_markers,
            warp_marker_count: header.warp_marker_count,
            range_edits: header.range_edits,
            range_edit_count: header.range_edit_count,
            note_store: header.note_store,
            note_frozen: header.note_frozen,
            processing_store: header.processing_store,
        });
    }

    scratch.link(save_only);
    Some(scratch)
}

unsafe fn copy_plugin_bytes(
    copy: unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8, usize) -> usize,
    host: *mut c_void,
    index: usize,
    plugin: usize,
) -> Vec<u8> {
    let length = unsafe { copy(host, index, plugin, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe { copy(host, index, plugin, data.as_mut_ptr(), length) };
    data.truncate(written.min(length));
    data
}

fn path_bytes(path: &[u8]) -> &std::path::Path {
    use std::os::unix::ffi::OsStrExt;
    std::path::Path::new(std::ffi::OsStr::from_bytes(path))
}

unsafe fn gather(
    host: *mut c_void,
    ops: &HirariLayoutHostOps,
    save_only: bool,
    cancel: Option<(unsafe extern "C" fn(*const c_void) -> bool, *const c_void)>,
) -> Option<Box<LayoutOwned>> {
    let cancelled = || {
        cancel.is_some_and(|(is_cancelled, context)| unsafe { is_cancelled(context) })
    };
    unsafe { (ops.begin)(host) };
    let gathered = (|| -> Option<Box<LayoutOwned>> {
        let timeline_sample_rate = unsafe { (ops.sample_rate)(host) };
        let track_count = unsafe { (ops.track_count)(host) };
        let mut scratch = Vec::with_capacity(track_count);
        for index in 0..track_count {
            if cancelled() {
                return None;
            }
            // A retired/null slot is skipped, not fatal.
            if let Some(track) =
                unsafe { gather_track(host, ops, index, save_only, timeline_sample_rate) }
            {
                scratch.push(track);
            }
        }
        let sidechain_routes =
            sidechain_route_list(unsafe { (ops.sidechain_routes_store)(host) });
        let feedback_routes = feedback_route_list(unsafe { (ops.routing_store)(host) });
        let mut tracks = Vec::with_capacity(scratch.len());
        for entry in &mut scratch {
            tracks.push(std::mem::take(&mut entry.view));
        }
        Some(Box::new(LayoutOwned {
            tracks: tracks.into_boxed_slice(),
            sidechain_routes,
            feedback_routes,
            timeline_sample_rate,
            _scratch: scratch,
        }))
    })();
    unsafe { (ops.end)(host) };
    gathered
}

/// Gathers the live engine model into a Rust-owned snapshot. Returns null when
/// the host cancelled or the model could not be read.
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_gather(
    host: *mut c_void,
    ops: *const HirariLayoutHostOps,
    save_only: bool,
    is_cancelled: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    cancel_context: *const c_void,
) -> *mut c_void {
    if host.is_null() || ops.is_null() {
        return std::ptr::null_mut();
    }
    let cancel = is_cancelled.map(|callback| (callback, cancel_context));
    let Some(owned) = (unsafe { gather(host, &*ops, save_only, cancel) }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(owned).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot(
    storage: *const c_void,
    out: *mut HirariProjectLayoutSnapshot,
) -> bool {
    if storage.is_null() || out.is_null() {
        return false;
    }
    unsafe { *out = (&*storage.cast::<LayoutOwned>()).snapshot() };
    true
}

/// Gathers the layout and serializes it to the Rust-owned JSON buffer in one
/// call, so C++ never holds a snapshot.
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_json_from_host(
    host: *mut c_void,
    ops: *const HirariLayoutHostOps,
    is_cancelled: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    cancel_context: *const c_void,
    json_data: *mut *mut u8,
    json_size: *mut usize,
) -> bool {
    if host.is_null() || ops.is_null() || json_data.is_null() || json_size.is_null() {
        return false;
    }
    let cancel = is_cancelled.map(|callback| (callback, cancel_context));
    let Some(owned) = (unsafe { gather(host, &*ops, false, cancel) }) else {
        return false;
    };
    let snapshot = owned.snapshot();
    let written = unsafe {
        hirari_project_layout_write_json(&snapshot, is_cancelled, cancel_context, json_data, json_size)
    };
    drop(owned);
    written
}

/// The saver path: gathers with the save-only fields and encodes the project
/// container directly, without a C++ snapshot in between.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_save_from_host(
    host: *mut c_void,
    ops: *const HirariLayoutHostOps,
    path: *const u8,
    path_len: usize,
    tempo_state: *const c_void,
    routing_state: *const c_void,
    arrangement_document: usize,
    bpm: f64,
    root_note: i32,
    scale_type: i32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let Some(owned) = (unsafe { gather(host, &*ops, true, None) }) else {
        return false;
    };
    let snapshot = owned.snapshot();
    let saved = unsafe {
        crate::project_save_layout::hirari_project_save_from_layout(
            path,
            path_len,
            &snapshot,
            tempo_state,
            routing_state,
            arrangement_document,
            bpm,
            root_note,
            scale_type,
        )
    };
    drop(owned);
    saved
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_release(storage: *mut c_void) {
    if !storage.is_null() {
        drop(unsafe { Box::from_raw(storage.cast::<LayoutOwned>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockTrack {
        scalars: HirariLayoutTrackScalars,
        name: Vec<u8>,
        states: Vec<Vec<u8>>,
        parameters: Vec<Vec<f32>>,
        gui: Vec<Vec<u8>>,
        region_path: Vec<u8>,
    }

    struct MockHost {
        tracks: Mutex<Vec<MockTrack>>,
        routes: *mut c_void,
        routing: *mut c_void,
        gauge: Mutex<(u32, u32)>,
    }

    impl Drop for MockHost {
        fn drop(&mut self) {
            unsafe {
                crate::sidechain_route_store::hirari_sidechain_routes_destroy(self.routes);
                crate::routing_engine_core::hirari_routing_engine_destroy(self.routing);
            }
        }
    }

    fn host_opaque(host: *mut c_void) -> &'static MockHost {
        unsafe { &*host.cast::<MockHost>() }
    }

    unsafe extern "C" fn begin(host: *mut c_void) {
        host_opaque(host).gauge.lock().unwrap().0 += 1;
    }
    unsafe extern "C" fn end(host: *mut c_void) {
        host_opaque(host).gauge.lock().unwrap().1 += 1;
    }
    unsafe extern "C" fn track_count(host: *mut c_void) -> usize {
        host_opaque(host).tracks.lock().unwrap().len()
    }
    unsafe extern "C" fn track_at(
        host: *mut c_void,
        index: usize,
        out: *mut HirariLayoutTrackScalars,
    ) -> bool {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        let Some(track) = tracks.get(index) else {
            return false;
        };
        unsafe { *out = track.scalars };
        true
    }
    fn copy(out: *mut u8, capacity: usize, bytes: &[u8]) -> usize {
        if !out.is_null() && capacity > 0 {
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, capacity.min(bytes.len())) };
        }
        bytes.len()
    }
    unsafe extern "C" fn track_string(
        host: *mut c_void,
        index: usize,
        kind: u32,
        out: *mut u8,
        capacity: usize,
    ) -> usize {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        let Some(track) = tracks.get(index) else {
            return 0;
        };
        match kind {
            0 => copy(out, capacity, &track.name),
            _ => 0,
        }
    }
    unsafe extern "C" fn automation(
        _: *mut c_void,
        _: usize,
        _: u32,
        _: *mut HirariLayoutAutomationPoint,
        _: usize,
    ) -> usize {
        0
    }
    unsafe extern "C" fn plugin_lane_count(_: *mut c_void, _: usize) -> usize {
        0
    }
    unsafe extern "C" fn plugin_lane_at(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: *mut HirariLayoutPluginLaneHeader,
    ) -> bool {
        false
    }
    unsafe extern "C" fn plugin_lane_points(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: *mut HirariLayoutAutomationPoint,
        _: usize,
    ) -> usize {
        0
    }
    unsafe extern "C" fn plugin_count(host: *mut c_void, index: usize) -> usize {
        host_opaque(host)
            .tracks
            .lock()
            .unwrap()
            .get(index)
            .map_or(0, |track| track.states.len())
    }
    unsafe extern "C" fn plugin_type(_: *mut c_void, _: usize, _: usize) -> u32 {
        1
    }
    unsafe extern "C" fn plugin_bypass(_: *mut c_void, _: usize, plugin: usize) -> bool {
        plugin == 0
    }
    unsafe extern "C" fn plugin_state(
        host: *mut c_void,
        index: usize,
        plugin: usize,
        out: *mut u8,
        capacity: usize,
    ) -> usize {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        let Some(track) = tracks.get(index) else {
            return 0;
        };
        track
            .states
            .get(plugin)
            .map_or(0, |state| copy(out, capacity, state))
    }
    unsafe extern "C" fn plugin_gui_state(
        host: *mut c_void,
        index: usize,
        plugin: usize,
        out: *mut u8,
        capacity: usize,
    ) -> usize {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        let Some(track) = tracks.get(index) else {
            return 0;
        };
        track
            .gui
            .get(plugin)
            .map_or(0, |state| copy(out, capacity, state))
    }
    unsafe extern "C" fn plugin_parameter_count(host: *mut c_void, index: usize, plugin: usize) -> usize {
        host_opaque(host)
            .tracks
            .lock()
            .unwrap()
            .get(index)
            .and_then(|track| track.parameters.get(plugin))
            .map_or(0, Vec::len)
    }
    unsafe extern "C" fn plugin_parameters(
        host: *mut c_void,
        index: usize,
        plugin: usize,
        out: *mut f32,
        capacity: usize,
    ) -> usize {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        let Some(values) = tracks
            .get(index)
            .and_then(|track| track.parameters.get(plugin))
        else {
            return 0;
        };
        if !out.is_null() && capacity > 0 {
            let count = capacity.min(values.len());
            unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), out, count) };
        }
        values.len()
    }
    unsafe extern "C" fn sandbox_count(_: *mut c_void, _: usize) -> usize {
        0
    }
    unsafe extern "C" fn sandbox_path(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: *mut u8,
        _: usize,
    ) -> usize {
        0
    }
    unsafe extern "C" fn sandbox_state(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: *mut u8,
        _: usize,
    ) -> usize {
        0
    }
    unsafe extern "C" fn region_count(_: *mut c_void, _: usize) -> usize {
        0
    }
    unsafe extern "C" fn region_at(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: *mut HirariLayoutRegionHeader,
    ) -> bool {
        false
    }
    unsafe extern "C" fn region_string(
        _: *mut c_void,
        _: usize,
        _: usize,
        _: u32,
        _: *mut u8,
        _: usize,
    ) -> usize {
        0
    }
    unsafe extern "C" fn sidechain_routes_store(host: *mut c_void) -> *mut c_void {
        host_opaque(host).routes
    }
    unsafe extern "C" fn routing_store(host: *mut c_void) -> *mut c_void {
        host_opaque(host).routing
    }
    unsafe extern "C" fn sample_rate(_: *mut c_void) -> f64 {
        48_000.0
    }

    fn ops() -> HirariLayoutHostOps {
        HirariLayoutHostOps {
            begin,
            end,
            track_count,
            track_at,
            track_string,
            automation,
            plugin_lane_count,
            plugin_lane_at,
            plugin_lane_points,
            plugin_count,
            plugin_type,
            plugin_bypass,
            plugin_state,
            plugin_gui_state,
            plugin_parameter_count,
            plugin_parameters,
            sandbox_count,
            sandbox_path,
            sandbox_state,
            region_count,
            region_at,
            region_string,
            sidechain_routes_store,
            routing_store,
            sample_rate,
        }
    }

    fn fixture() -> Box<MockHost> {
        let routes = crate::sidechain_route_store::hirari_sidechain_routes_create();
        let routing = crate::routing_engine_core::hirari_routing_engine_create();
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(routes, 1, 2, 3, 1);
            crate::routing_engine_core::hirari_routing_engine_add_feedback(routing, 1, 2, 0.5);
        }
        Box::new(MockHost {
            tracks: Mutex::new(vec![MockTrack {
                scalars: HirariLayoutTrackScalars {
                    id: 7,
                    track_type: 1,
                    volume: 0.5,
                    record_armed: true,
                    ..Default::default()
                },
                name: b"Bass".to_vec(),
                states: vec![vec![1, 2], vec![3]],
                parameters: vec![vec![0.25, 0.75], vec![]],
                gui: vec![vec![9], vec![8]],
                region_path: b"/nonexistent/hirari-region.wav".to_vec(),
            }]),
            routes,
            routing,
            gauge: Mutex::new((0, 0)),
        })
    }

    unsafe fn snapshot_of(host: &MockHost) -> (HirariProjectLayoutSnapshot, *mut c_void) {
        let ops = ops();
        let storage = hirari_layout_gather(
            (&*host as *const MockHost).cast_mut().cast(),
            &ops,
            false,
            None,
            std::ptr::null(),
        );
        if storage.is_null() {
            return (HirariProjectLayoutSnapshot::default(), storage);
        }
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        (snapshot, storage)
    }

    #[test]
    fn gather_copies_scalars_strings_and_plugins() {
        let host = fixture();
        let (snapshot, storage) = unsafe { snapshot_of(&host) };
        assert_eq!(snapshot.track_count, 1);
        assert_eq!(snapshot.sidechain_route_count, 1);
        assert_eq!(snapshot.feedback_route_count, 1);
        assert_eq!(snapshot.timeline_sample_rate, 48_000.0);
        let track = unsafe { &*snapshot.tracks };
        assert_eq!(track.id, 7);
        assert_eq!(track.volume, 0.5);
        assert!(track.record_armed);
        let name = unsafe { std::slice::from_raw_parts(track.name.data, track.name.len) };
        assert_eq!(name, b"Bass");
        assert_eq!(track.plugin_type_count, 2);
        assert_eq!(unsafe { *track.plugin_bypass }, 1);
        assert_eq!(track.plugin_state_count, 2);
        let state = unsafe { &*track.plugin_states.add(1) };
        assert_eq!(unsafe { std::slice::from_raw_parts(state.bytes, state.count) }, &[3]);
        // The JSON path fills parameters (plug-in order) and no GUI blobs.
        let values = unsafe { &*track.plugin_parameters.add(1) };
        assert_eq!(values.count, 0);
        let first = unsafe { &*track.plugin_parameters };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(first.values, first.count) },
            &[0.25, 0.75]
        );
        assert!(track.plugin_gui_states.is_null());
        unsafe { hirari_layout_release(storage) };
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn save_only_gather_fills_gui_states_instead_of_parameters() {
        let host = fixture();
        let ops = ops();
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &ops,
                true,
                None,
                std::ptr::null(),
            )
        };
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        let track = unsafe { &*snapshot.tracks };
        assert!(track.plugin_parameters.is_null());
        assert_eq!(track.plugin_gui_state_count, 2);
        let gui = unsafe { &*track.plugin_gui_states };
        assert_eq!(unsafe { std::slice::from_raw_parts(gui.bytes, gui.count) }, &[9]);
        let second = unsafe { &*track.plugin_gui_states.add(1) };
        assert_eq!(unsafe { std::slice::from_raw_parts(second.bytes, second.count) }, &[8]);
        unsafe { hirari_layout_release(storage) };
    }

    #[test]
    fn cancellation_releases_the_host_lock() {
        unsafe extern "C" fn cancelled(_: *const c_void) -> bool {
            true
        }
        let host = fixture();
        let ops = ops();
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &ops,
                false,
                Some(cancelled),
                std::ptr::null(),
            )
        };
        assert!(storage.is_null());
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn null_inputs_are_rejected() {
        let ops = ops();
        assert!(unsafe {
            hirari_layout_gather(std::ptr::null_mut(), &ops, false, None, std::ptr::null())
        }
        .is_null());
        assert!(!unsafe { hirari_layout_snapshot(std::ptr::null(), std::ptr::null_mut()) });
        unsafe { hirari_layout_release(std::ptr::null_mut()) };
    }

    #[test]
    fn source_rate_is_clamped_to_the_supported_range() {
        assert_eq!(clamped_source_rate(96_000.0, 48_000.0), 96_000);
        assert_eq!(clamped_source_rate(0.0, 44_100.0), 44_100);
        assert_eq!(clamped_source_rate(1.0, 48_000.0), 8_000);
        assert_eq!(clamped_source_rate(1_000_000.0, 48_000.0), 384_000);
    }
}
