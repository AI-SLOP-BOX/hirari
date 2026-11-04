//! Project-layout gathering.
//!
//! Every field of a track already lives in a Rust storage; C++ contributes only
//! the track's identity, its name and handles to those storages, so this module
//! owns every snapshot rule — string ownership, automation copying, plug-in
//! ordering, region rate clamping, missing-file detection. The result owns all
//! copied blocks and stays valid until the JSON writer or the project saver has
//! consumed it.

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

/// The scalars a track reports, gathered from its channel strip, realtime
/// runtime and freeze store.
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

/// The track's own Rust storages, read here through the `track_storage` op.
const STRIP: u32 = 0;
const CHAIN: u32 = 1;
const AUTOMATION: u32 = 2;
const FROZEN: u32 = 3;
const RUNTIME: u32 = 4;
const ARTICULATION: u32 = 5;

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

/// Host accessors. `begin` flushes recorded automation and takes the tracks
/// lock so the borrowed region pointers stay stable for the whole gather;
/// `end` always runs, including on cancellation. `track_handle` is the identity
/// every other read keys on — the region table lives in a Rust store C++ keys
/// by that same pointer — and `track_storage` hands out the Rust storages that
/// hold the rest of the snapshot.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariLayoutHostOps {
    pub begin: unsafe extern "C" fn(*mut c_void),
    pub end: unsafe extern "C" fn(*mut c_void),
    pub track_count: unsafe extern "C" fn(*mut c_void) -> usize,
    pub track_handle: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    pub track_id: unsafe extern "C" fn(*mut c_void) -> u32,
    pub track_type: unsafe extern "C" fn(*mut c_void) -> u32,
    pub track_storage: unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void,
    pub track_name: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize,
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

/// Reads a byte string from one of the host's own accessors in two passes.
unsafe fn host_string(
    read: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize,
    source: *mut c_void,
) -> Vec<u8> {
    let length = unsafe { read(source, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe { read(source, data.as_mut_ptr(), length) };
    data.truncate(written.min(length));
    data
}

/// Asks a Rust store for one of its byte strings in two passes: length, then
/// content.
unsafe fn store_string(
    read: unsafe extern "C" fn(*mut c_void, usize, u32, *mut u8, usize) -> usize,
    key: *mut c_void,
    index: usize,
    kind: u32,
) -> Vec<u8> {
    let length = unsafe { read(key, index, kind, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe { read(key, index, kind, data.as_mut_ptr(), length) };
    data.truncate(written.min(length));
    data
}

/// Reads a plug-in state buffer, which the chain allocates and this frees.
fn chain_bytes(chain: *mut c_void, index: u32, gui: bool) -> Vec<u8> {
    let mut length = 0usize;
    let data = unsafe {
        if gui {
            crate::effect_chain_host::hirari_effect_chain_processor_gui_state(
                chain, index, &mut length,
            )
        } else {
            crate::effect_chain_host::hirari_effect_chain_processor_state(
                chain, index, &mut length,
            )
        }
    };
    if data.is_null() || length == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length).to_vec() };
    unsafe { crate::effect_chain_host::hirari_effect_chain_buffer_free(data, length) };
    bytes
}

/// Copies one sandbox slot's bundle path out of the chain.
fn sandbox_path(chain: *mut c_void, slot: usize) -> Vec<u8> {
    let length =
        unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(chain, slot, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut data = vec![0u8; length];
    let written = unsafe {
        crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(
            chain,
            slot,
            data.as_mut_ptr(),
            length,
        )
    };
    data.truncate(written.min(length));
    data
}

/// Copies one expression map, which the articulation runtime clones for the
/// caller and this releases.
fn expression_map(articulation: *mut c_void, kind: u8) -> Vec<u8> {
    let mut length = 0usize;
    let data = unsafe {
        crate::track_midi_articulation::hirari_track_midi_expression_map_take(
            articulation, kind, &mut length,
        )
    };
    if data.is_null() || length == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length).to_vec() };
    unsafe {
        crate::track_midi_articulation::hirari_track_midi_expression_map_take_free(data, length)
    };
    bytes
}

fn owned_points(
    raw: *const crate::track_automation_store::AutomationPoint,
    count: usize,
) -> Vec<HirariLayoutAutomationPoint> {
    if raw.is_null() || count == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(raw, count) }
        .iter()
        .map(|point| HirariLayoutAutomationPoint {
            time: point.time,
            value: point.value,
            curve: point.curve,
        })
        .collect()
}

fn track_points(store: *mut c_void, kind: u32) -> Vec<HirariLayoutAutomationPoint> {
    let mut count = 0usize;
    let raw =
        unsafe { crate::track_automation_store::hirari_track_automation_store_points(store, kind, &mut count) };
    owned_points(raw, count)
}

fn plugin_lanes(store: *mut c_void) -> Vec<LaneScratch> {
    let mut count = 0usize;
    let views =
        unsafe { crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(store, &mut count) };
    if views.is_null() || count == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(views, count) }
        .iter()
        .map(|lane| LaneScratch {
            plugin_index: lane.processor_index,
            parameter_id: lane.parameter_id,
            points: owned_points(lane.points, lane.point_count),
        })
        .collect()
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
    let track = unsafe { (ops.track_handle)(host, index) };
    if track.is_null() {
        return None;
    }
    let storage = |kind: u32| unsafe { (ops.track_storage)(track, kind) };
    let strip = storage(STRIP);
    let chain = storage(CHAIN);
    let automation = storage(AUTOMATION);
    let frozen = storage(FROZEN);
    let runtime = storage(RUNTIME);
    let articulation = storage(ARTICULATION);
    let scalars = HirariLayoutTrackScalars {
        id: unsafe { (ops.track_id)(track) },
        track_type: unsafe { (ops.track_type)(track) },
        volume: unsafe { crate::channel_strip::hirari_channel_strip_gain(strip) },
        pan: unsafe { crate::channel_strip::hirari_channel_strip_pan(strip) },
        muted: unsafe { crate::track_runtime::hirari_track_runtime_is_muted(runtime) },
        solo: unsafe { crate::channel_strip::hirari_channel_strip_is_solo(strip) },
        record_armed: unsafe {
            crate::channel_strip::hirari_channel_strip_is_record_armed(strip)
        },
        phase_inverted: unsafe {
            crate::track_runtime::hirari_track_runtime_is_phase_inverted(runtime)
        },
        track_delay_samples: unsafe {
            crate::track_runtime::hirari_track_runtime_track_delay_samples(runtime)
        } as u64,
        frozen: unsafe { crate::track_frozen_store::hirari_track_frozen_store_is_frozen(frozen) },
        frozen_total_samples: unsafe {
            crate::track_frozen_store::hirari_track_frozen_store_total_samples(frozen)
        },
        frozen_sample_rate: unsafe {
            crate::track_frozen_store::hirari_track_frozen_store_sample_rate(frozen)
        },
        spatial_x: unsafe { crate::track_runtime::hirari_track_runtime_spatial(runtime, 0) },
        spatial_y: unsafe { crate::track_runtime::hirari_track_runtime_spatial(runtime, 1) },
        spatial_z: unsafe { crate::track_runtime::hirari_track_runtime_spatial(runtime, 2) },
    };
    let frozen_length =
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_path_len(frozen) };
    let mut frozen_path = vec![0u8; frozen_length];
    if frozen_length > 0 {
        let written = unsafe {
            crate::track_frozen_store::hirari_track_frozen_store_path_copy(
                frozen,
                frozen_path.as_mut_ptr(),
                frozen_length,
            )
        };
        frozen_path.truncate(written.min(frozen_length));
    }
    let mut scratch = TrackScratch {
        scalars,
        name: unsafe { host_string(ops.track_name, track) },
        frozen_path,
        expression_map: expression_map(articulation, 0),
        expression_map_pro: expression_map(articulation, 1),
        volume_automation: track_points(automation, 0),
        pan_automation: track_points(automation, 1),
        track_delay_automation: track_points(automation, 2),
        lane_storage: plugin_lanes(automation),
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
    let plugin_count = unsafe { crate::effect_chain_host::hirari_effect_chain_count(chain) };
    scratch.plugin_types.reserve(plugin_count as usize);
    scratch.plugin_bypass.reserve(plugin_count as usize);
    for plugin in 0..plugin_count {
        scratch
            .plugin_types
            .push(unsafe { crate::effect_chain_host::hirari_effect_chain_node_kind(chain, plugin) });
        scratch.plugin_bypass.push(
            if unsafe { crate::effect_chain_host::hirari_effect_chain_bypass(chain, plugin) } {
                1
            } else {
                0
            },
        );
        scratch.plugin_states.push(chain_bytes(chain, plugin, false));
        if save_only {
            scratch.plugin_gui_states.push(chain_bytes(chain, plugin, true));
        } else {
            let count =
                unsafe { crate::effect_chain_host::hirari_effect_chain_parameter_count(chain, plugin) };
            scratch.plugin_parameters.push(
                (0..count)
                    .map(|parameter| {
                        unsafe {
                            crate::effect_chain_host::hirari_effect_chain_parameter(
                                chain, plugin, parameter,
                            )
                        }
                    })
                    .collect(),
            );
        }
    }

    let slot_count =
        unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_count(chain) };
    for slot in 0..slot_count {
        scratch.sandbox_paths.push(sandbox_path(chain, slot));
        let plugin =
            unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_index(chain, slot) };
        scratch.sandbox_states.push(chain_bytes(chain, plugin, false));
    }

    let region_count = crate::region_store::hirari_region_store_count(track);
    scratch.regions.reserve(region_count);
    scratch.region_strings.reserve(region_count * 2);
    for region_index in 0..region_count {
        let mut header = HirariLayoutRegionHeader::default();
        if !unsafe {
            crate::region_store::hirari_region_store_layout_header(track, region_index, &mut header)
        } {
            continue;
        }
        let name = unsafe {
            store_string(
                crate::region_store::hirari_region_store_layout_string,
                track,
                region_index,
                0,
            )
        };
        let path = unsafe {
            store_string(
                crate::region_store::hirari_region_store_layout_string,
                track,
                region_index,
                1,
            )
        };
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

/// One background layout snapshot: the engine the worker reads, the table it
/// reads it through, and the flag the UI raises when it loses interest.
struct LayoutSession {
    engine: *mut crate::ffi::HirariUnifiedEngine,
    ops: *const HirariLayoutHostOps,
    cancelled: std::sync::atomic::AtomicBool,
}

// Only the snapshot worker dereferences the engine, and the owning
// `AudioEngine` joins that worker before its graph can be destroyed.
unsafe impl Send for LayoutSession {}

struct RecordedLayout {
    before: u64,
    after: u64,
    layout: Vec<u8>,
}

extern "C" fn session_cancelled(context: *const c_void) -> bool {
    let session = context.cast::<LayoutSession>();
    !session.is_null() && unsafe { (*session).cancelled.load(std::sync::atomic::Ordering::Acquire) }
}

extern "C" fn session_set_cancelled(context: *mut c_void) {
    let session = context.cast::<LayoutSession>();
    if !session.is_null() {
        unsafe { (*session).cancelled.store(true, std::sync::atomic::Ordering::Release) };
    }
}

/// Gathers and serializes the layout, bracketing it with the project state
/// revision so the poller can tell a snapshot that aged out from a usable one.
extern "C" fn session_compute(context: *mut c_void, output: *mut *mut c_void) -> bool {
    let (Some(session), Some(output)) = (unsafe {
        context.cast::<LayoutSession>().as_mut()
    }, unsafe { output.as_mut() })
    else {
        return false;
    };
    if session.cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let before = unsafe { (*session.engine).get_project_state_revision() };
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0;
    if !unsafe {
        hirari_layout_json_from_host(
            session.engine.cast::<c_void>(),
            session.ops,
            Some(session_cancelled),
            (session as *mut LayoutSession).cast::<c_void>(),
            &mut data,
            &mut length,
        )
    } {
        return false;
    }
    let layout = if data.is_null() || length == 0 {
        Vec::new()
    } else {
        unsafe { Vec::from_raw_parts(data, length, length) }
    };
    let after = unsafe { (*session.engine).get_project_state_revision() };
    *output = Box::into_raw(Box::new(RecordedLayout {
        before,
        after,
        layout,
    }))
    .cast();
    true
}

extern "C" fn session_read(
    result: *mut c_void,
    before: *mut u64,
    after: *mut u64,
    data: *mut *const u8,
    length: *mut usize,
) -> bool {
    let Some(recorded) = (unsafe { result.cast::<RecordedLayout>().as_ref() }) else {
        return false;
    };
    if before.is_null() || after.is_null() || data.is_null() || length.is_null() {
        return false;
    }
    unsafe {
        *before = recorded.before;
        *after = recorded.after;
        *data = recorded.layout.as_ptr();
        *length = recorded.layout.len();
    }
    true
}

extern "C" fn session_destroy_result(result: *mut c_void) {
    if !result.is_null() {
        drop(unsafe { Box::from_raw(result.cast::<RecordedLayout>()) });
    }
}

extern "C" fn session_destroy_context(context: *mut c_void) {
    if !context.is_null() {
        drop(unsafe { Box::from_raw(context.cast::<LayoutSession>()) });
    }
}

extern "C" fn visit_snapshot_bytes(context: *mut c_void, bytes: *const u8, length: usize) {
    let Some(output) = (unsafe { context.cast::<Vec<u8>>().as_mut() }) else {
        return;
    };
    if bytes.is_null() && length != 0 {
        return;
    }
    output.clear();
    if length != 0 {
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
    }
}

/// Snapshot owners are keyed by engine address, so a session belongs to exactly
/// one engine and a reused address starts a clean owner.
static LAYOUT_OWNERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, u64>>,
> = std::sync::OnceLock::new();

fn layout_owners() -> &'static std::sync::Mutex<std::collections::HashMap<usize, u64>> {
    LAYOUT_OWNERS.get_or_init(std::sync::Mutex::default)
}

fn owner_for(host: usize) -> u64 {
    let mut owners = layout_owners().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(owner) = owners.get(&host) {
        return *owner;
    }
    let owner = crate::async_snapshots::hirari_layout_snapshot_owner_create();
    owners.insert(host, owner);
    owner
}

fn existing_owner(host: usize) -> Option<u64> {
    layout_owners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&host)
        .copied()
}

/// Retires the snapshots of one engine and joins its worker. The native
/// `AudioEngine` destructor calls this before its graph can be released.
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot_owner_release_host(host: *mut c_void) {
    let owner = (!host.is_null())
        .then(|| {
            layout_owners()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(host as usize))
        })
        .flatten();
    if let Some(owner) = owner {
        crate::async_snapshots::release_layout_owner(owner);
    }
}

fn ops_for(
    engine: *const crate::ffi::HirariUnifiedEngine,
) -> *const HirariLayoutHostOps {
    if engine.is_null() {
        return std::ptr::null();
    }
    unsafe { (*engine).layout_ops_handle() as *const HirariLayoutHostOps }
}

/// The live project layout as JSON. The gather is Rust; C++ contributes the
/// engine host, its ops table and the track names.
pub fn project_layout_json(core: *const crate::ffi::HirariUnifiedEngine) -> String {
    let ops = ops_for(core);
    if ops.is_null() {
        return String::new();
    }
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0;
    if !unsafe {
        hirari_layout_json_from_host(
            core.cast_mut().cast::<c_void>(),
            ops,
            None,
            std::ptr::null(),
            &mut data,
            &mut length,
        )
    } || data.is_null()
        || length == 0
    {
        return String::new();
    }
    let bytes = unsafe { Vec::from_raw_parts(data, length, length) };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Starts the background save's layout snapshot and returns its request id.
pub fn request_layout_snapshot(engine: *mut crate::ffi::HirariUnifiedEngine) -> u64 {
    let ops = ops_for(engine);
    if ops.is_null() {
        return 0;
    }
    let owner = owner_for(engine as usize);
    let session = Box::into_raw(Box::new(LayoutSession {
        engine,
        ops,
        cancelled: std::sync::atomic::AtomicBool::new(false),
    }))
    .cast::<c_void>();
    unsafe {
        crate::async_snapshots::hirari_layout_snapshot_request(
            owner,
            session,
            Some(session_compute),
            Some(session_read),
            Some(session_destroy_result),
            Some(session_destroy_context),
            Some(session_set_cancelled),
            Some(session_cancelled),
        )
    }
}

/// Empty while the snapshot is still being prepared, otherwise
/// `READY <revision>\n<layout-json>` or a failure marker.
pub fn poll_layout_snapshot(
    engine: *const crate::ffi::HirariUnifiedEngine,
    request_id: u64,
) -> String {
    if request_id == 0 {
        return "ERROR".to_owned();
    }
    let owner = existing_owner(engine as usize).unwrap_or(0);
    let mut bytes = Vec::new();
    let status = unsafe {
        crate::async_snapshots::hirari_layout_snapshot_poll(
            request_id,
            owner,
            (&mut bytes as *mut Vec<u8>).cast::<c_void>(),
            Some(visit_snapshot_bytes),
        )
    };
    match status {
        0 => String::new(),
        1 => String::from_utf8_lossy(&bytes).into_owned(),
        _ => "ERROR".to_owned(),
    }
}

pub fn cancel_layout_snapshot(
    engine: *const crate::ffi::HirariUnifiedEngine,
    request_id: u64,
) -> bool {
    if request_id == 0 {
        return false;
    }
    let Some(owner) = existing_owner(engine as usize) else {
        return false;
    };
    unsafe { crate::async_snapshots::hirari_layout_snapshot_cancel(request_id, owner) }
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
    use crate::channel_strip::{
        hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_set_gain,
        hirari_channel_strip_set_pan, hirari_channel_strip_set_record_armed,
        hirari_channel_strip_set_solo,
    };
    use crate::effect_chain_host::{
        hirari_effect_chain_add_builtin, hirari_effect_chain_create, hirari_effect_chain_destroy,
        hirari_effect_chain_parameter, hirari_effect_chain_parameter_count,
        hirari_effect_chain_set_bypass,
    };
    use crate::track_automation_store::{
        hirari_track_automation_store_create, hirari_track_automation_store_destroy,
        hirari_track_automation_store_set_plugin_lanes, hirari_track_automation_store_set_points,
        AutomationPoint, PluginLaneView,
    };
    use crate::track_frozen_store::{
        hirari_track_frozen_store_create, hirari_track_frozen_store_destroy,
        hirari_track_frozen_store_publish, TrackFrozenOps,
    };
    use crate::track_midi_articulation::{
        hirari_track_midi_articulation_create, hirari_track_midi_articulation_destroy,
        hirari_track_midi_expression_map_set,
    };
    use crate::track_runtime::{
        hirari_track_runtime_create, hirari_track_runtime_destroy, hirari_track_runtime_set_delays,
        hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
        hirari_track_runtime_set_spatial,
    };
    use std::sync::Mutex;

    /// A track assembled from the same Rust storages the engine hands over, so
    /// the gather is tested against the reads it really performs.
    struct MockTrack {
        id: u32,
        track_type: u32,
        name: Vec<u8>,
        strip: *mut c_void,
        chain: *mut c_void,
        automation: *mut c_void,
        frozen: *mut c_void,
        runtime: *mut c_void,
        articulation: *mut c_void,
    }

    impl Drop for MockTrack {
        fn drop(&mut self) {
            unsafe {
                hirari_track_runtime_destroy(self.runtime);
                hirari_track_midi_articulation_destroy(self.articulation);
                hirari_track_frozen_store_destroy(self.frozen);
                hirari_track_automation_store_destroy(self.automation);
                hirari_effect_chain_destroy(self.chain);
                hirari_channel_strip_destroy(self.strip);
            }
        }
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

    fn mock_track(track: *mut c_void) -> &'static MockTrack {
        unsafe { &*track.cast::<MockTrack>() }
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
    unsafe extern "C" fn track_handle(host: *mut c_void, index: usize) -> *mut c_void {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        tracks
            .get(index)
            .map_or(std::ptr::null_mut(), |track| track as *const MockTrack as *mut c_void)
    }
    unsafe extern "C" fn track_id(track: *mut c_void) -> u32 {
        if track.is_null() {
            return 0;
        }
        mock_track(track).id
    }
    unsafe extern "C" fn track_type(track: *mut c_void) -> u32 {
        if track.is_null() {
            return 0;
        }
        mock_track(track).track_type
    }
    unsafe extern "C" fn track_storage(track: *mut c_void, kind: u32) -> *mut c_void {
        if track.is_null() {
            return std::ptr::null_mut();
        }
        let track = mock_track(track);
        match kind {
            STRIP => track.strip,
            CHAIN => track.chain,
            AUTOMATION => track.automation,
            FROZEN => track.frozen,
            RUNTIME => track.runtime,
            _ => track.articulation,
        }
    }
    unsafe extern "C" fn track_name(track: *mut c_void, out: *mut u8, capacity: usize) -> usize {
        if track.is_null() {
            return 0;
        }
        let name = &mock_track(track).name;
        if !out.is_null() && capacity > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(name.as_ptr(), out, capacity.min(name.len()));
            }
        }
        name.len()
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
            track_handle,
            track_id,
            track_type,
            track_storage,
            track_name,
            sidechain_routes_store,
            routing_store,
            sample_rate,
        }
    }

    unsafe extern "C" fn release_frozen_owner(_: *mut c_void) {}

    fn fixture() -> Box<MockHost> {
        let routes = crate::sidechain_route_store::hirari_sidechain_routes_create();
        let routing = crate::routing_engine_core::hirari_routing_engine_create();
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(routes, 1, 2, 3, 1);
            crate::routing_engine_core::hirari_routing_engine_add_feedback(routing, 1, 2, 0.5);
        }
        let strip = hirari_channel_strip_create();
        let chain = hirari_effect_chain_create();
        let automation = hirari_track_automation_store_create();
        let frozen_ops = TrackFrozenOps {
            destroy_owner: release_frozen_owner,
        };
        let frozen = unsafe { hirari_track_frozen_store_create(&frozen_ops) };
        let articulation = hirari_track_midi_articulation_create();
        let null = std::ptr::null_mut();
        let runtime = unsafe {
            hirari_track_runtime_create(
                7, chain, strip, automation, frozen, null, null, null, null, null, null, null,
                null, None, null,
            )
        };
        let volume_points = [
            AutomationPoint {
                time: 1.0,
                value: 0.25,
                curve: 0.0,
            },
            AutomationPoint {
                time: 2.0,
                value: 0.75,
                curve: 0.5,
            },
        ];
        let lane_points = [AutomationPoint {
            time: 3.0,
            value: 0.5,
            curve: 0.0,
        }];
        let lanes = [PluginLaneView {
            processor_index: 1,
            parameter_id: 2,
            points: lane_points.as_ptr(),
            point_count: lane_points.len(),
        }];
        let frozen_path = b"/tmp/hirari-frozen.wav";
        unsafe {
            hirari_channel_strip_set_gain(strip, 0.5);
            hirari_channel_strip_set_pan(strip, -0.25);
            hirari_channel_strip_set_solo(strip, true);
            hirari_channel_strip_set_record_armed(strip, true);
            hirari_track_runtime_set_muted(runtime, 1);
            hirari_track_runtime_set_phase_invert(runtime, 1);
            hirari_track_runtime_set_delays(runtime, 12, 34);
            hirari_track_runtime_set_spatial(runtime, 0.1, 0.2, 0.3);
            hirari_track_automation_store_set_points(
                automation,
                0,
                volume_points.as_ptr(),
                volume_points.len(),
            );
            hirari_track_automation_store_set_plugin_lanes(automation, lanes.as_ptr(), lanes.len());
            assert!(hirari_effect_chain_add_builtin(chain, 1, false, false));
            assert!(hirari_effect_chain_add_builtin(chain, 4, false, false));
            assert!(hirari_effect_chain_set_bypass(chain, 0, true));
            assert!(hirari_track_frozen_store_publish(
                frozen,
                1 as *mut c_void,
                2 as *mut c_void,
                2,
                4,
                4,
                48_000,
                7,
                3,
                frozen_path.as_ptr(),
                frozen_path.len(),
            ));
            assert!(hirari_track_midi_expression_map_set(
                articulation,
                0,
                b"{}".as_ptr(),
                2
            ));
        }
        Box::new(MockHost {
            tracks: Mutex::new(vec![MockTrack {
                id: 7,
                track_type: 1,
                name: b"Bass".to_vec(),
                strip,
                chain,
                automation,
                frozen,
                runtime,
                articulation,
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

    fn text(value: &HirariLayoutString) -> &[u8] {
        if value.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(value.data, value.len) }
    }

    #[test]
    fn gather_reads_the_tracks_own_rust_storages() {
        let host = fixture();
        let chain = host.tracks.lock().unwrap()[0].chain;
        let (snapshot, storage) = unsafe { snapshot_of(&host) };
        assert_eq!(snapshot.track_count, 1);
        assert_eq!(snapshot.sidechain_route_count, 1);
        assert_eq!(snapshot.feedback_route_count, 1);
        assert_eq!(snapshot.timeline_sample_rate, 48_000.0);
        let track = unsafe { &*snapshot.tracks };
        assert_eq!(track.id, 7);
        assert_eq!(track.track_type, 1);
        assert_eq!(track.volume, 0.5);
        assert_eq!(track.pan, -0.25);
        assert!(track.solo);
        assert!(track.record_armed);
        // Mute, phase and timing are the runtime's; freeze is the store's.
        assert!(track.muted);
        assert!(track.phase_inverted);
        assert_eq!(track.track_delay_samples, 34);
        assert_eq!(
            (track.spatial_x, track.spatial_y, track.spatial_z),
            (0.1, 0.2, 0.3)
        );
        assert!(track.frozen);
        assert_eq!(track.frozen_total_samples, 4);
        assert_eq!(track.frozen_sample_rate, 48_000);
        assert_eq!(text(&track.name), b"Bass");
        assert_eq!(text(&track.frozen_path), b"/tmp/hirari-frozen.wav");
        assert_eq!(text(&track.expression_map_json), b"{}");
        assert_eq!(text(&track.expression_map_pro_json), b"null");
        let volume =
            unsafe { std::slice::from_raw_parts(track.volume_automation, track.volume_automation_count) };
        assert_eq!(
            volume
                .iter()
                .map(|point| (point.time, point.value, point.curve))
                .collect::<Vec<_>>(),
            vec![(1.0, 0.25, 0.0), (2.0, 0.75, 0.5)]
        );
        assert_eq!(track.pan_automation_count, 0);
        assert_eq!(track.plugin_automation_count, 1);
        let lane = unsafe { &*track.plugin_automation };
        assert_eq!((lane.plugin_index, lane.parameter_id, lane.point_count), (1, 2, 1));
        let lane_point = unsafe { &*lane.points };
        assert_eq!((lane_point.time, lane_point.value), (3.0, 0.5));
        assert_eq!(track.plugin_type_count, 2);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(track.plugin_types, track.plugin_type_count) },
            &[1, 4]
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(track.plugin_bypass, track.plugin_type_count) },
            &[1, 0]
        );
        assert_eq!(track.plugin_state_count, 2);
        assert_eq!(track.sandbox_plugin_path_count, 0);
        assert!(track.plugin_gui_states.is_null());
        for plugin in 0..2 {
            let state = unsafe { &*track.plugin_states.add(plugin) };
            assert_eq!(
                unsafe { std::slice::from_raw_parts(state.bytes, state.count) },
                chain_bytes(chain, plugin as u32, false)
            );
            // The JSON path reports the plug-in's parameters in id order.
            let block = unsafe { &*track.plugin_parameters.add(plugin) };
            let values =
                unsafe { std::slice::from_raw_parts(block.values, block.count) };
            assert_eq!(
                values
                    .iter()
                    .enumerate()
                    .map(|(id, value)| (id as u32, *value))
                    .collect::<Vec<_>>(),
                (0..unsafe { hirari_effect_chain_parameter_count(chain, plugin as u32) })
                    .map(|id| (id, unsafe {
                        hirari_effect_chain_parameter(chain, plugin as u32, id)
                    }))
                    .collect::<Vec<_>>()
            );
        }
        unsafe { hirari_layout_release(storage) };
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn save_only_gather_fills_gui_states_instead_of_parameters() {
        let host = fixture();
        let chain = host.tracks.lock().unwrap()[0].chain;
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
        for plugin in 0..2 {
            let gui = unsafe { &*track.plugin_gui_states.add(plugin) };
            assert_eq!(
                unsafe { std::slice::from_raw_parts(gui.bytes, gui.count) },
                chain_bytes(chain, plugin as u32, true)
            );
        }
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
    fn retired_track_slots_are_skipped() {
        unsafe extern "C" fn retired_handle(_: *mut c_void, _: usize) -> *mut c_void {
            std::ptr::null_mut()
        }
        let host = fixture();
        let mut table = ops();
        table.track_handle = retired_handle;
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &table,
                false,
                None,
                std::ptr::null(),
            )
        };
        assert!(!storage.is_null());
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        assert_eq!(snapshot.track_count, 0);
        unsafe { hirari_layout_release(storage) };
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn source_rate_is_clamped_to_the_supported_range() {
        assert_eq!(clamped_source_rate(96_000.0, 48_000.0), 96_000);
        assert_eq!(clamped_source_rate(0.0, 44_100.0), 44_100);
        assert_eq!(clamped_source_rate(1.0, 48_000.0), 8_000);
        assert_eq!(clamped_source_rate(1_000_000.0, 48_000.0), 384_000);
    }
}
