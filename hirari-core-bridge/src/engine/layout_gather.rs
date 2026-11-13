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
/// The plug-in chain storage kind, so another host walk can read a track's
/// sandboxed processors through the same table.
pub const CHAIN: u32 = 1;
const AUTOMATION: u32 = 2;
const FROZEN: u32 = 3;
const RUNTIME: u32 = 4;
const ARTICULATION: u32 = 5;
const NAME: u32 = 6;

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
    /// The sidechain route table and the routing graph are Rust stores the
    /// engine only holds; Rust reads them through its own store API.
    pub sidechain_routes_store: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub routing_store: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub sample_rate: unsafe extern "C" fn(*mut c_void) -> f64,
}

/// One slot of the bag the given engine host created.
fn host_store(host: *const c_void, kind: u32) -> *mut c_void {
    crate::engine_session::handle_of_host(host, kind)
}

/// The live graph of the given engine host as a borrowed window, valid while
/// the tracks lock is held.
fn host_tracks(host: *const c_void) -> (*const *mut c_void, usize) {
    let table = host_store(host, crate::engine_session::H_TRACK_TABLE);
    unsafe {
        (
            crate::engine_track_table::hirari_engine_track_ptrs(table),
            crate::engine_track_table::hirari_engine_track_count(table),
        )
    }
}

/// Takes the tracks lock and drains the state a snapshot must not race: the
/// automation the recorder is still holding and the chain nodes a released
/// effect has not handed back.
unsafe extern "C" fn native_begin(host: *mut c_void) {
    crate::track_session::lock_tracks(host);
    let (items, count) = host_tracks(host);
    if items.is_null() {
        return;
    }
    for index in 0..count {
        let track = unsafe { *items.add(index) };
        if track.is_null() {
            continue;
        }
        unsafe {
            crate::engine_track::hirari_track_flush_recorded_automation(track);
            crate::engine_track::hirari_track_reclaim_chain(track);
        }
    }
}

unsafe extern "C" fn native_end(host: *mut c_void) {
    crate::track_session::unlock_tracks(host);
}

unsafe extern "C" fn native_track_count(host: *mut c_void) -> usize {
    host_tracks(host).1
}

unsafe extern "C" fn native_track_handle(host: *mut c_void, index: usize) -> *mut c_void {
    let (items, count) = host_tracks(host);
    if items.is_null() || index >= count {
        return std::ptr::null_mut();
    }
    unsafe { *items.add(index) }
}

unsafe extern "C" fn native_sidechain_routes(host: *mut c_void) -> *mut c_void {
    host_store(host, crate::engine_session::H_SIDECHAIN_ROUTES)
}

unsafe extern "C" fn native_routing(host: *mut c_void) -> *mut c_void {
    host_store(host, crate::engine_session::H_ROUTING)
}

unsafe extern "C" fn native_sample_rate(host: *mut c_void) -> f64 {
    unsafe {
        crate::engine_block_host::hirari_engine_block_device_sample_rate(host_store(
            host,
            crate::engine_session::H_ENGINE_BLOCK,
        ))
    }
}

/// The gather's own view of a live engine. Every accessor resolves the host to
/// the session bag that engine created and reads the Rust store behind it, so
/// the engine contributes the tracks lock and nothing else.
static NATIVE_OPS: HirariLayoutHostOps = HirariLayoutHostOps {
    begin: native_begin,
    end: native_end,
    track_count: native_track_count,
    track_handle: native_track_handle,
    track_id: crate::engine_track::hirari_track_id,
    track_type: crate::engine_track::hirari_track_type,
    track_storage: crate::engine_track::hirari_track_storage,
    sidechain_routes_store: native_sidechain_routes,
    routing_store: native_routing,
    sample_rate: native_sample_rate,
};

/// One storage of the track with this id (kind 0 is the channel strip), read
/// while the graph is locked. `None` when the engine has no such track.
pub fn with_track_storage<R>(
    host: *const c_void,
    track_id: u32,
    kind: u32,
    read: impl FnOnce(*mut c_void) -> R,
) -> Option<R> {
    let host = host.cast_mut();
    unsafe { native_begin(host) };
    let (items, count) = unsafe { host_tracks(host) };
    let track = (0..count)
        .map(|index| unsafe { *items.add(index) })
        .find(|track| {
            !track.is_null() && unsafe { crate::engine_track::hirari_track_id(*track) } == track_id
        });
    let found = track.and_then(|track| {
        let storage = unsafe { crate::engine_track::hirari_track_storage(track, kind) };
        (!storage.is_null()).then(|| read(storage))
    });
    unsafe { native_end(host) };
    found
}

/// Visits one storage of every track in engine order with the graph locked for
/// the whole walk, so the slots stay where the realtime callback left them.
pub fn each_track_storage(host: *const c_void, kind: u32, mut visit: impl FnMut(u32, *mut c_void)) {
    let host = host.cast_mut();
    unsafe { native_begin(host) };
    let (items, count) = unsafe { host_tracks(host) };
    for index in 0..count {
        let track = unsafe { *items.add(index) };
        if track.is_null() {
            continue;
        }
        let storage = unsafe { crate::engine_track::hirari_track_storage(track, kind) };
        if !storage.is_null() {
            visit(unsafe { crate::engine_track::hirari_track_id(track) }, storage);
        }
    }
    unsafe { native_end(host) };
}

pub(crate) struct LaneScratch {
    plugin_index: u32,
    parameter_id: u32,
    points: Vec<HirariLayoutAutomationPoint>,
}

pub(crate) struct TrackScratch {
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
#[path = "layout_gather_owned.rs"]
mod owned;
pub use owned::*;
#[path = "layout_gather_public.rs"]
mod public_;
pub use public_::*;

#[cfg(test)]
#[path = "layout_gather_tests.rs"]
mod layout_gather_tests;

