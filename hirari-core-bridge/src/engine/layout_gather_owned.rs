//! Layout-owned snapshots: the fully-owned outcome of a layout gather,
//! plus the two-pass string copies and the region/plugin tables that
//! belong to no single track, split out of layout_gather.rs.
use super::*;

pub(crate) struct LayoutOwned {
    tracks: Box<[HirariLayoutTrack]>,
    sidechain_routes: Vec<HirariLayoutSidechainRoute>,
    feedback_routes: Vec<HirariLayoutFeedbackRoute>,
    timeline_sample_rate: f64,
    // Keeps every borrowed block the tracks point at alive.
    _scratch: Vec<TrackScratch>,
}

impl LayoutOwned {
    pub(crate) fn snapshot(&self) -> HirariProjectLayoutSnapshot {
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
/// Asks a Rust store for one of its byte strings in two passes: length, then
/// content.
pub(crate) unsafe fn store_string(
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
pub(crate) fn chain_bytes(chain: *mut c_void, index: u32, gui: bool) -> Vec<u8> {
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
pub(crate) fn sandbox_path(chain: *mut c_void, slot: usize) -> Vec<u8> {
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
pub(crate) fn expression_map(articulation: *mut c_void, kind: u8) -> Vec<u8> {
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

pub(crate) fn owned_points(
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

pub(crate) fn track_points(store: *mut c_void, kind: u32) -> Vec<HirariLayoutAutomationPoint> {
    let mut count = 0usize;
    let raw =
        unsafe { crate::track_automation_store::hirari_track_automation_store_points(store, kind, &mut count) };
    owned_points(raw, count)
}

pub(crate) fn plugin_lanes(store: *mut c_void) -> Vec<LaneScratch> {
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

pub(crate) fn clamped_source_rate(raw: f64, timeline: f64) -> u32 {
    let value = if raw > 0.0 { raw } else { timeline };
    let rounded = if value.is_finite() { value.round() as i64 } else { 0 };
    rounded.clamp(8_000, 384_000) as u32
}

pub(crate) unsafe fn gather_track(
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
        name: crate::track_name_store::read(storage(NAME) as *const c_void),
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

pub(crate) fn path_bytes(path: &[u8]) -> &std::path::Path {
    use std::os::unix::ffi::OsStrExt;
    std::path::Path::new(std::ffi::OsStr::from_bytes(path))
}

pub(crate) unsafe fn gather(
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
/// call, so C++ never holds a snapshot. The engine host is only a key: the walk
/// reads the stores the host's session bag owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_json_from_host(
    host: *mut c_void,
    is_cancelled: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    cancel_context: *const c_void,
    json_data: *mut *mut u8,
    json_size: *mut usize,
) -> bool {
    if host.is_null() || json_data.is_null() || json_size.is_null() {
        return false;
    }
    let cancel = is_cancelled.map(|callback| (callback, cancel_context));
    let Some(owned) = (unsafe { gather(host, &NATIVE_OPS, false, cancel) }) else {
        return false;
    };
    let snapshot = owned.snapshot();
    let written = unsafe {
        hirari_project_layout_write_json(&snapshot, is_cancelled, cancel_context, json_data, json_size)
    };
    drop(owned);
    written
}

/// The live layout as a fingerprint. Command admission hashes it rather than
/// materializing a string, so a stale command is rejected on the generation the
/// graph actually has.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_generation(host: *mut c_void) -> u64 {
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    if unsafe { hirari_layout_json_from_host(host, None, std::ptr::null(), &mut data, &mut length) }
        && !data.is_null()
    {
        unsafe {
            crate::project_layout::hirari_project_layout_fingerprint_update(&mut hash, data, length)
        };
    }
    unsafe { crate::project_layout::hirari_project_layout_json_free(data, length) };
    hash
}

/// The monotonic edit token the UI polls. It is the mark the engine block's
/// layout revision was bound to, so a save or a command can compare it without
/// gathering anything.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_revision(host: *const c_void) -> u64 {
    let block = host_store(host, crate::engine_session::H_ENGINE_BLOCK);
    let revision = unsafe { crate::engine_block_host::hirari_engine_block_layout_revision(block) };
    unsafe { crate::engine_track::hirari_layout_revision_current(revision) }
}

/// The layout token, the drained-MIDI revision and the control revision mixed
/// into one number, so a background snapshot can tell a usable result from one
/// that aged out while it was being prepared.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_state_revision(host: *const c_void) -> u64 {
    let layout = unsafe { hirari_project_layout_revision(host) };
    let midi = unsafe {
        crate::midi_note_edit::hirari_midi_snapshot_store_revision(host_store(
            host,
            crate::engine_session::H_MIDI_SNAPSHOT_STORE,
        ))
    };
    let control = unsafe {
        crate::engine_block_host::hirari_engine_block_control_revision(host_store(
            host,
            crate::engine_session::H_ENGINE_BLOCK,
        ))
    };
    crate::project_layout::hirari_project_state_revision_mix(layout, midi, control)
}
