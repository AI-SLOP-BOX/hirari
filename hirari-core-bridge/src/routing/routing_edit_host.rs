//! Host-driven routing and track-state edits.
//!
//! The routing graph and the sidechain route table are Rust stores that the
//! engine only holds, so this module reads and mutates them through its own
//! store API. It owns the edit policy: decide whether an edit is legal, apply
//! the resolved mutation, and record exactly one undo entry whose payloads
//! replay through this same module. Track state arrives the same way every other
//! control-side edit reads it — through the shared `TrackSession`, which writes
//! the track's own Rust channel strip and runtime while the engine lock is held.
//! Sidechain registration derives its inputs the same way: the device format
//! from the engine block state and the source's work buffer through the
//! session. Host callbacks remain only for the store addresses the engine holds
//! and the two operations that really are engine code — the layout mark and
//! the graph resync.
//!
//! Every callback runs on the control thread. Nothing here is reachable from
//! the audio callback.

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::channel_strip::{
    hirari_channel_strip_gain, hirari_channel_strip_is_record_armed, hirari_channel_strip_is_solo,
    hirari_channel_strip_pan, hirari_channel_strip_set_gain, hirari_channel_strip_set_pan,
    hirari_channel_strip_set_record_armed, hirari_channel_strip_set_solo,
};
use crate::plugin_edit_host::{register_replay, UndoStateFn};
use crate::track_runtime::{
    hirari_track_runtime_is_muted, hirari_track_runtime_is_phase_inverted,
    hirari_track_runtime_pdc_requested, hirari_track_runtime_set_delays,
    hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
    hirari_track_runtime_track_delay_samples,
};
use crate::track_session::TrackSession;

/// Matches `RoutingEngine::kMaxNodes`; ids at or above it are not routable.
const MAX_ROUTING_NODES: u32 = 128;

/// Payload ops. `src/core/rust_ffi.hpp` declares the payload layout the undo
/// journal stores, but only Rust interprets these values.
pub const HIRARI_ROUTING_EDIT_ROUTE: i32 = 1;
pub const HIRARI_ROUTING_EDIT_ROUTE_GAIN: i32 = 2;
pub const HIRARI_ROUTING_EDIT_SEND: i32 = 3;
pub const HIRARI_ROUTING_EDIT_FEEDBACK: i32 = 4;
pub const HIRARI_ROUTING_EDIT_SIDECHAIN: i32 = 5;
pub const HIRARI_ROUTING_EDIT_ARM: i32 = 6;
pub const HIRARI_ROUTING_EDIT_PHASE: i32 = 7;
pub const HIRARI_ROUTING_EDIT_DELAY: i32 = 8;
pub const HIRARI_ROUTING_EDIT_VOLUME: i32 = 9;
pub const HIRARI_ROUTING_EDIT_PAN: i32 = 10;
pub const HIRARI_ROUTING_EDIT_MUTE: i32 = 11;
pub const HIRARI_ROUTING_EDIT_SOLO: i32 = 12;

/// One reversible mutation. The host applies the payload without re-deciding;
/// sidechain payloads are the exception because their previous state is
/// re-derived (the C++ undo call went through the same validation).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRoutingEditPayload {
    pub op: i32,
    pub source_id: u32,
    pub dest_id: u32,
    pub plugin_index: u32,
    pub tap_point: u32,
    pub gain: f32,
    pub samples: u32,
    pub enabled: bool,
    pub pre_fader: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HirariRoutingEdgeState {
    pub route_enabled: bool,
    pub route_gain: f32,
    pub send_gain: f32,
    pub send_pre_fader: bool,
    pub feedback_gain: f32,
}

impl Default for HirariRoutingEdgeState {
    fn default() -> Self {
        Self {
            route_enabled: false,
            route_gain: 0.0,
            send_gain: 0.0,
            send_pre_fader: false,
            // A negative feedback gain means "edge absent".
            feedback_gain: -1.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRoutingTrackState {
    pub armed: bool,
    pub phase_inverted: bool,
    pub is_bus: bool,
    pub delay_samples: u32,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HirariRoutingSidechainEntry {
    pub source_id: u32,
    pub dest_id: u32,
    pub plugin_index: u32,
    pub tap_point: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRoutingSidechainContext {
    pub block_size: u32,
    pub sample_rate: u32,
    pub generation: u64,
    pub source_found: bool,
    pub destination_found: bool,
    pub source_prepared: bool,
}

pub type SidechainContextFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut HirariRoutingSidechainContext) -> bool;
/// `silent` selects the manager's silent registration path for sources that
/// are not prepared yet.
pub type RegisterSidechainFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    *const HirariRoutingSidechainContext,
    u32,
    bool,
) -> bool;
/// Hands the Rust-owned store handle to the edit host. The engine holds the
/// routing graph, the sidechain route table and the sidechain gate but never
/// interprets them.
pub type StoreFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
pub type HostFn = unsafe extern "C" fn(*mut c_void);
pub type RecordUndoFn = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const HirariRoutingEditPayload,
    usize,
    *const HirariRoutingEditPayload,
    usize,
    bool,
    bool,
);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariRoutingHostOps {
    /// The routing graph and the sidechain route table are Rust stores the
    /// engine only holds, so the edit host reads and writes them directly.
    pub routing_store: Option<StoreFn>,
    pub sidechain_routes_store: Option<StoreFn>,
    pub sidechain_engine_store: Option<StoreFn>,
    pub sidechain_context: Option<SidechainContextFn>,
    pub register_sidechain: Option<RegisterSidechainFn>,
    pub mark_layout_changed: Option<HostFn>,
    /// Rebuilds the published track/PDC graph; takes the tracks lock itself.
    pub sync_structural: Option<HostFn>,
    pub record_undo: Option<RecordUndoFn>,
    pub undo_state: Option<UndoStateFn>,
    /// The engine block state, whose recorded device format feeds sidechain
    /// registration when the host supplies no context/registration callbacks.
    /// The native engine installs this and leaves the two callbacks null;
    /// a unit-test table may install the callbacks instead.
    pub engine_block_store: Option<StoreFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariRoutingHostOps {}
unsafe impl Sync for HirariRoutingHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariRoutingHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariRoutingHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_host_install(ops: *const HirariRoutingHostOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

fn route_payload(
    op: i32,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
    pre_fader: bool,
) -> HirariRoutingEditPayload {
    HirariRoutingEditPayload {
        op,
        source_id,
        dest_id,
        gain,
        enabled,
        pre_fader,
        ..Default::default()
    }
}

fn sidechain_payload(
    source_id: u32,
    dest_id: u32,
    plugin_index: u32,
    tap_point: u32,
    enabled: bool,
) -> HirariRoutingEditPayload {
    HirariRoutingEditPayload {
        op: HIRARI_ROUTING_EDIT_SIDECHAIN,
        source_id,
        dest_id,
        plugin_index,
        tap_point,
        enabled,
        ..Default::default()
    }
}

fn track_payload(op: i32, track_id: u32, enabled: bool, samples: u32) -> HirariRoutingEditPayload {
    HirariRoutingEditPayload {
        op,
        source_id: track_id,
        samples,
        enabled,
        ..Default::default()
    }
}

fn track_mix_payload(op: i32, track_id: u32, value: f32) -> HirariRoutingEditPayload {
    HirariRoutingEditPayload {
        op,
        source_id: track_id,
        gain: value,
        ..Default::default()
    }
}

fn routing_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.routing_store?;
    Some(unsafe { access(host) })
}

fn sidechain_routes_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.sidechain_routes_store?;
    Some(unsafe { access(host) })
}

/// Removes one link from both Rust stores that describe it: the sidechain gate
/// holds the live audio routing, the route table the saved model.
fn remove_sidechain(ops: &HirariRoutingHostOps, host: *mut c_void, dest: u32, plugin: u32) -> bool {
    let gate_removed = match ops.sidechain_engine_store {
        Some(access) => unsafe {
            crate::sidechain_manager::hirari_sidechain_engine_remove_link(
                access(host),
                dest,
                plugin,
            )
        },
        None => false,
    };
    let Some(routes) = sidechain_routes_store(ops, host) else {
        return gate_removed;
    };
    let route_removed = unsafe {
        crate::sidechain_route_store::hirari_sidechain_routes_remove_link(routes, dest, plugin)
    };
    gate_removed || route_removed
}

fn query_edge(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingEdgeState> {
    let routing = routing_store(ops, host)?;
    Some(unsafe {
        HirariRoutingEdgeState {
            route_enabled: crate::routing_engine_core::hirari_routing_engine_has_connection(
                routing, source_id, dest_id,
            ),
            route_gain: crate::routing_engine_core::hirari_routing_engine_connection_gain(
                routing, source_id, dest_id,
            ),
            send_gain: crate::routing_engine_core::hirari_routing_engine_send_gain(
                routing, source_id, dest_id,
            ),
            send_pre_fader: crate::routing_engine_core::hirari_routing_engine_send_pre_fader(
                routing, source_id, dest_id,
            ),
            feedback_gain: crate::routing_engine_core::hirari_routing_engine_feedback_gain(
                routing, source_id, dest_id,
            ),
        }
    })
}

/// One track's routing-relevant state, read from the stores the track owns:
/// the fader, pan, solo and record-arm live in its channel strip, mute, phase
/// and the manual timing offset in its runtime, and the bus flag is the
/// engine's own track type.
fn track_state(host: *mut c_void, track_id: u32) -> Option<HirariRoutingTrackState> {
    let session = TrackSession::open(host, track_id)?;
    let strip = session.strip();
    let runtime = session.runtime();
    Some(HirariRoutingTrackState {
        armed: unsafe { hirari_channel_strip_is_record_armed(strip) },
        phase_inverted: unsafe { hirari_track_runtime_is_phase_inverted(runtime) },
        is_bus: session.is_bus(),
        delay_samples: unsafe { hirari_track_runtime_track_delay_samples(runtime) },
        volume: unsafe { hirari_channel_strip_gain(strip) },
        pan: unsafe { hirari_channel_strip_pan(strip) },
        muted: unsafe { hirari_track_runtime_is_muted(runtime) },
        solo: unsafe { hirari_channel_strip_is_solo(strip) },
    })
}

/// Applies one track-state mutation. A gate change — mute, solo, phase — drops
/// the frozen render because it changes what the track outputs; record-arm only
/// bumps the layout revision since nothing audible moves; any other op is the
/// manual timing offset, which bumps the layout only when the runtime accepted
/// a different value than it held.
fn apply_track_scalar(
    host: *mut c_void,
    track_id: u32,
    op: i32,
    enabled: bool,
    samples: u32,
) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let (strip, runtime) = (session.strip(), session.runtime());
    match op {
        HIRARI_ROUTING_EDIT_ARM => {
            session.commit_layout(unsafe { hirari_channel_strip_set_record_armed(strip, enabled) })
        }
        HIRARI_ROUTING_EDIT_PHASE => {
            session.invalidate_frozen();
            unsafe { hirari_track_runtime_set_phase_invert(runtime, enabled as u8) };
            session.mark_layout();
            true
        }
        HIRARI_ROUTING_EDIT_MUTE => {
            session.invalidate_frozen();
            unsafe { hirari_track_runtime_set_muted(runtime, enabled as u8) };
            session.mark_layout();
            true
        }
        HIRARI_ROUTING_EDIT_SOLO => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_solo(strip, enabled) };
            session.mark_layout();
            true
        }
        _ => {
            let previous = unsafe { hirari_track_runtime_track_delay_samples(runtime) };
            unsafe {
                hirari_track_runtime_set_delays(
                    runtime,
                    hirari_track_runtime_pdc_requested(runtime),
                    samples,
                )
            };
            // The runtime bounds the offset to its shared delay line, so the
            // accepted value is what decides whether the graph has to move.
            session.commit_layout(
                unsafe { hirari_track_runtime_track_delay_samples(runtime) } != previous,
            );
            true
        }
    }
}

/// Applies one fader or pan edit, dropping the frozen render first because the
/// mix the track renders changes with it.
fn apply_track_mix(host: *mut c_void, track_id: u32, op: i32, value: f32) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let strip = session.strip();
    match op {
        HIRARI_ROUTING_EDIT_VOLUME => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_gain(strip, value) };
        }
        HIRARI_ROUTING_EDIT_PAN => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_pan(strip, value) };
        }
        _ => return false,
    }
    session.mark_layout();
    true
}

fn list_sidechain(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
) -> Option<Vec<HirariRoutingSidechainEntry>> {
    let routes = sidechain_routes_store(ops, host)?;
    let count = unsafe { crate::sidechain_route_store::hirari_sidechain_routes_count(routes) };
    let mut buffer = vec![
        crate::sidechain_route_store::HirariSidechainRoute {
            source_id: 0,
            destination_id: 0,
            plugin_index: 0,
            tap_point: 0,
        };
        count
    ];
    if count > 0 {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_copy(
                routes,
                buffer.as_mut_ptr(),
                count,
            );
        }
    }
    Some(
        buffer
            .into_iter()
            .map(|route| HirariRoutingSidechainEntry {
                source_id: route.source_id,
                dest_id: route.destination_id,
                plugin_index: route.plugin_index,
                tap_point: route.tap_point,
            })
            .collect(),
    )
}

fn sidechain_engine_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.sidechain_engine_store?;
    Some(unsafe { access(host) })
}

fn engine_block_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.engine_block_store?;
    let state = unsafe { access(host) };
    (!state.is_null()).then_some(state)
}

/// The sidechain registration inputs, read from the state the engine actually
/// owns: the device format recorded by the last graph preparation, the
/// generation the buffers were published at, and the source's own work buffer
/// for the "prepared at this block size" check. The Rust sidechain storage
/// holds 4096 frames, so the request is clamped before any source is tapped.
fn sidechain_context_native(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingSidechainContext> {
    let engine_block = engine_block_store(ops, host)?;
    let block_size =
        unsafe { crate::engine_block_host::hirari_engine_block_device_block_frames(engine_block) }
            .min(4096);
    let sample_rate = unsafe {
        crate::engine_block_host::hirari_engine_block_device_sample_rate(engine_block)
    }
    .max(1.0) as u32;
    let generation =
        unsafe { crate::engine_block_host::hirari_engine_block_generation(engine_block) }.max(1);
    let (source_found, source_prepared) = match TrackSession::open(host, source_id) {
        Some(session) => {
            let tapped = unsafe {
                crate::audio_buffer_ops::hirari_audio_buffer_storage_samples(session.work_buffer())
            } >= block_size;
            (true, tapped)
        }
        None => (false, false),
    };
    let destination_found = TrackSession::open(host, dest_id).is_some();
    Some(HirariRoutingSidechainContext {
        block_size,
        sample_rate,
        generation,
        source_found,
        destination_found,
        source_prepared: source_prepared && destination_found,
    })
}

fn sidechain_context(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingSidechainContext> {
    match ops.sidechain_context {
        Some(query) => {
            let mut context = HirariRoutingSidechainContext::default();
            unsafe { query(host, source_id, dest_id, &mut context) }.then_some(context)
        }
        None => sidechain_context_native(ops, host, source_id, dest_id),
    }
}

/// Registers one sidechain tap with the gate. A source whose work buffer has
/// not reached the current block size is registered silent, so the plug-in can
/// gate on the model without reading unprepared audio; a prepared source taps
/// its own work buffer storage.
fn register_sidechain_native(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    dest_id: u32,
    plugin_index: u32,
    source_id: u32,
    context: &HirariRoutingSidechainContext,
    tap_point: u32,
    silent: bool,
) -> bool {
    let Some(gate) = sidechain_engine_store(ops, host) else {
        return false;
    };
    if silent {
        return unsafe {
            crate::sidechain_manager::hirari_sidechain_engine_register_silent_link(
                gate,
                dest_id,
                plugin_index,
                source_id,
                context.block_size,
                context.sample_rate,
                context.generation,
                1.0,
                tap_point as u8,
            )
        };
    }
    let Some(session) = TrackSession::open(host, source_id) else {
        return false;
    };
    let work = session.work_buffer();
    let channels = unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_channels(work) };
    let left = unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(work, 0, 0) };
    let right = if channels > 1 {
        unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(work, 1, 0) }
    } else {
        left
    };
    // Registration copies one prepared block into Rust-owned storage;
    // supplying the frame count is part of that contract.
    unsafe {
        crate::sidechain_manager::hirari_sidechain_engine_register_link(
            gate,
            dest_id,
            plugin_index,
            source_id,
            left,
            right,
            context.block_size,
            context.sample_rate,
            context.generation,
            1.0,
            tap_point as u8,
        )
    }
}

fn build_graph(ops: &HirariRoutingHostOps, host: *mut c_void) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    // The native host callback ignored the topology result, so a rebuild is
    // reported as done; cycle admission is decided before the dependency is
    // added.
    unsafe { crate::routing_engine_core::hirari_routing_engine_build(routing) };
    true
}

fn mark_layout_changed(ops: &HirariRoutingHostOps, host: *mut c_void) {
    if let Some(mark) = ops.mark_layout_changed {
        unsafe { mark(host) };
    }
}

fn add_dependency(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> bool {
    match routing_store(ops, host) {
        Some(routing) => unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_dependency(
                routing, source_id, dest_id,
            )
        },
        None => false,
    }
}

fn remove_dependency(ops: &HirariRoutingHostOps, host: *mut c_void, source_id: u32, dest_id: u32) {
    if let Some(routing) = routing_store(ops, host) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_remove_dependency(
                routing, source_id, dest_id,
            );
        }
    }
}

/// Rebuilds the routing graph and republishes the derived track/PDC state.
fn publish_routes(ops: &HirariRoutingHostOps, host: *mut c_void) {
    if let Some(routing) = routing_store(ops, host) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_build(routing);
        }
    }
    if let Some(sync) = ops.sync_structural {
        unsafe { sync(host) };
    }
}

/// Applies one resolved route/send mutation to the Rust-owned graph. A route
/// without a saved gain is a unity route; a disabled entry removes the edge.
fn apply_route_payload(ops: &HirariRoutingHostOps, host: *mut c_void, edit: &HirariRoutingEditPayload) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    unsafe {
        if edit.op == HIRARI_ROUTING_EDIT_SEND {
            if edit.enabled {
                crate::routing_engine_core::hirari_routing_engine_add_connection(
                    routing,
                    edit.source_id,
                    edit.dest_id,
                    edit.gain,
                    true,
                    edit.pre_fader,
                );
            } else {
                crate::routing_engine_core::hirari_routing_engine_remove_send(
                    routing, edit.source_id, edit.dest_id,
                );
            }
        } else if edit.enabled {
            crate::routing_engine_core::hirari_routing_engine_add_connection(
                routing,
                edit.source_id,
                edit.dest_id,
                if edit.gain > 0.0 { edit.gain } else { 1.0 },
                false,
                false,
            );
        } else {
            crate::routing_engine_core::hirari_routing_engine_remove_connection(
                routing, edit.source_id, edit.dest_id,
            );
        }
    }
    true
}

/// A negative feedback gain names the absent edge, so it removes it.
fn apply_feedback(ops: &HirariRoutingHostOps, host: *mut c_void, source_id: u32, dest_id: u32, gain: f32) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    unsafe {
        if gain < 0.0 {
            crate::routing_engine_core::hirari_routing_engine_remove_feedback(
                routing, source_id, dest_id,
            );
            return true;
        }
        crate::routing_engine_core::hirari_routing_engine_add_feedback(
            routing, source_id, dest_id, gain,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn record_undo(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRoutingEditPayload],
    redo: &[HirariRoutingEditPayload],
    perform: bool,
    coalesce: bool,
) {
    if let Some(record) = ops.record_undo {
        unsafe {
            record(
                host,
                name.as_ptr() as *const c_char,
                undo.as_ptr(),
                undo.len(),
                redo.as_ptr(),
                redo.len(),
                perform,
                coalesce,
            );
        }
        return;
    }
    record_in_history(ops, host, name, undo, redo, perform, coalesce);
}

unsafe fn replay_payloads(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    entries: &[HirariRoutingEditPayload],
) {
    apply_payloads_with(ops, host, entries);
}

fn record_in_history(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRoutingEditPayload],
    redo: &[HirariRoutingEditPayload],
    perform: bool,
    coalesce: bool,
) {
    let Some(read_state) = ops.undo_state else {
        return;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return;
    }
    let ops_copy = *ops;
    let undo_copy = undo.to_vec();
    let redo_copy = redo.to_vec();
    let undo_id = register_replay(
        state,
        Box::new(move || unsafe { replay_payloads(&ops_copy, host, &undo_copy) }),
    );
    if undo_id == 0 {
        return;
    }
    let ops_copy = *ops;
    let redo_id = register_replay(
        state,
        Box::new(move || unsafe { replay_payloads(&ops_copy, host, &redo_copy) }),
    );
    if redo_id == 0 {
        unsafe {
            crate::undo_transaction::hirari_undo_manager_discard_native_callback(state, undo_id)
        };
        return;
    }
    let name = name.strip_suffix(&[0]).unwrap_or(name);
    unsafe {
        crate::undo_transaction::hirari_undo_manager_commit(
            state,
            name.as_ptr(),
            name.len(),
            undo_id,
            redo_id,
            coalesce,
            perform,
        );
    }
}

fn valid_route_gain(gain: f32, enabled: bool) -> bool {
    gain.is_finite() && (0.0..=2.0).contains(&gain) && !(enabled && gain <= 0.0)
}

/// `set_route`: the enabled flag is the whole edit. Undo restores the prior
/// route gain as well so removing and re-adding an edge is lossless.
fn set_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES || dest_id >= MAX_ROUTING_NODES || source_id == dest_id {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if before.route_enabled == enabled {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_ROUTE, source_id, dest_id, 1.0, enabled, false),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if after.route_enabled != enabled {
        return false;
    }
    let before_enabled = before.route_enabled;
    record_undo(
        ops,
        host,
        b"Audio Route\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE,
            source_id,
            dest_id,
            before.route_gain,
            before_enabled,
            false,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE,
            source_id,
            dest_id,
            if enabled { 1.0 } else { before.route_gain },
            enabled,
            false,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

fn set_route_gain_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !valid_route_gain(gain, enabled)
    {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if before.route_enabled == enabled && (!enabled || (before.route_gain - gain).abs() < 1.0e-6) {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_ROUTE_GAIN, source_id, dest_id, gain, enabled, false),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if after.route_enabled != enabled {
        return false;
    }
    record_undo(
        ops,
        host,
        b"Audio Route Gain\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE_GAIN,
            source_id,
            dest_id,
            before.route_gain,
            before.route_enabled,
            false,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE_GAIN,
            source_id,
            dest_id,
            gain,
            enabled,
            false,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

fn set_send_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    pre_fader: bool,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !valid_route_gain(gain, enabled)
    {
        return false;
    }
    if enabled {
        if track_state(host, source_id).is_none() {
            return false;
        }
        let Some(destination) = track_state(host, dest_id) else {
            return false;
        };
        // Sends resolve to buses only; a track-to-track edge is an insert.
        if !destination.is_bus {
            return false;
        }
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    let before_enabled = before.send_gain > 0.0;
    if before_enabled == enabled
        && (!enabled
            || ((before.send_gain - gain).abs() < 1.0e-6 && before.send_pre_fader == pre_fader))
    {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_SEND, source_id, dest_id, gain, enabled, pre_fader),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if (after.send_gain > 0.0) != enabled {
        return false;
    }
    record_undo(
        ops,
        host,
        b"Audio Send\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_SEND,
            source_id,
            dest_id,
            before.send_gain,
            before_enabled,
            before.send_pre_fader,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_SEND,
            source_id,
            dest_id,
            gain,
            enabled,
            pre_fader,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

fn set_feedback_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !gain.is_finite()
    {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if enabled {
        if !apply_feedback(ops, host, source_id, dest_id, gain) {
            return false;
        }
    } else {
        if before.feedback_gain < 0.0 {
            return false;
        }
        apply_feedback(ops, host, source_id, dest_id, -1.0);
    }
    mark_layout_changed(ops, host);
    record_undo(
        ops,
        host,
        b"Feedback Route\0",
        &[HirariRoutingEditPayload {
            op: HIRARI_ROUTING_EDIT_FEEDBACK,
            source_id,
            dest_id,
            gain: before.feedback_gain,
            ..Default::default()
        }],
        &[HirariRoutingEditPayload {
            op: HIRARI_ROUTING_EDIT_FEEDBACK,
            source_id,
            dest_id,
            gain: if enabled { gain.clamp(0.0, 2.0) } else { -1.0 },
            ..Default::default()
        }],
        false,
        false,
    );
    true
}

/// True when `target` is reachable from `from` following sidechain edges
/// (source -> destination). Mirrors the C++ depth-first walk.
fn sidechain_reaches(routes: &[HirariRoutingSidechainEntry], from: u32, target: u32) -> bool {
    let mut pending = vec![from];
    let mut visited: Vec<u32> = Vec::new();
    while let Some(node) = pending.pop() {
        if node == target {
            return true;
        }
        if visited.contains(&node) {
            continue;
        }
        visited.push(node);
        for route in routes {
            if route.source_id == node {
                pending.push(route.dest_id);
            }
        }
    }
    false
}

fn set_sidechain_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    plugin_index: u32,
    tap_point: u32,
    enabled: bool,
    record: bool,
) -> bool {
    if source_id == dest_id || tap_point > 2 {
        return false;
    }
    let Some(routes) = list_sidechain(ops, host) else {
        return false;
    };
    let previous = routes
        .iter()
        .find(|route| route.dest_id == dest_id && route.plugin_index == plugin_index)
        .copied();
    let had_previous = previous.is_some();
    let previous_source = previous.map_or(0, |route| route.source_id);
    let previous_tap = previous.map_or(2, |route| route.tap_point);
    let edge_used_by_another = |source: u32| {
        routes.iter().any(|route| {
            route.source_id == source
                && route.dest_id == dest_id
                && route.plugin_index != plugin_index
        })
    };

    if !enabled {
        let removed = remove_sidechain(ops, host, dest_id, plugin_index);
        if had_previous && !edge_used_by_another(previous_source) {
            remove_dependency(ops, host, previous_source, dest_id);
        }
        if !removed {
            return false;
        }
        build_graph(ops, host);
        mark_layout_changed(ops, host);
        if record {
            record_undo(
                ops,
                host,
                b"Sidechain Disconnect\0",
                &[sidechain_payload(
                    previous_source,
                    dest_id,
                    plugin_index,
                    previous_tap,
                    true,
                )],
                &[sidechain_payload(
                    previous_source,
                    dest_id,
                    plugin_index,
                    previous_tap,
                    false,
                )],
                false,
                false,
            );
        }
        return true;
    }

    // Sidechain dependencies are directed source -> destination. Adding a
    // link is rejected when the destination can already reach the source;
    // this prevents A->B->A cycles before the native graph is published.
    if sidechain_reaches(&routes, dest_id, source_id) {
        return false;
    }
    let Some(context) = sidechain_context(ops, host, source_id, dest_id) else {
        return false;
    };
    if !context.source_found || !context.destination_found {
        return false;
    }
    let new_edge_used =
        edge_used_by_another(source_id) || (had_previous && previous_source == source_id);
    let removed_previous_dependency =
        had_previous && previous_source != source_id && !edge_used_by_another(previous_source);
    if removed_previous_dependency {
        remove_dependency(ops, host, previous_source, dest_id);
    }
    let restore_previous_dependency = |ops: &HirariRoutingHostOps, host: *mut c_void| {
        if removed_previous_dependency {
            add_dependency(ops, host, previous_source, dest_id);
            build_graph(ops, host);
        }
    };
    if !add_dependency(ops, host, source_id, dest_id) || !build_graph(ops, host) {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    }
    let silent = !context.source_prepared;
    let registered = match ops.register_sidechain {
        Some(register) => unsafe {
            register(
                host,
                dest_id,
                plugin_index,
                source_id,
                &context,
                tap_point,
                silent,
            )
        },
        None => register_sidechain_native(
            ops,
            host,
            dest_id,
            plugin_index,
            source_id,
            &context,
            tap_point,
            silent,
        ),
    };
    if !registered {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    }
    if let Some(routes) = sidechain_routes_store(ops, host) {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(
                routes,
                source_id,
                dest_id,
                plugin_index,
                tap_point,
            )
        };
    }
    mark_layout_changed(ops, host);
    if record {
        let undo = if had_previous {
            sidechain_payload(previous_source, dest_id, plugin_index, previous_tap, true)
        } else {
            sidechain_payload(0, dest_id, plugin_index, 0, false)
        };
        record_undo(
            ops,
            host,
            b"Sidechain Connect\0",
            &[undo],
            &[sidechain_payload(
                source_id,
                dest_id,
                plugin_index,
                tap_point,
                true,
            )],
            false,
            false,
        );
    }
    true
}

fn set_track_armed_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    armed: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    // An already-correct value is a valid idempotent application.
    if track.armed == armed {
        return true;
    }
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_ARM, armed, 0) {
        return false;
    }
    if record {
        record_undo(
            ops,
            host,
            b"Record Arm\0",
            &[track_payload(HIRARI_ROUTING_EDIT_ARM, track_id, !armed, 0)],
            &[track_payload(HIRARI_ROUTING_EDIT_ARM, track_id, armed, 0)],
            false,
            false,
        );
    }
    true
}

fn set_phase_invert_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    inverted: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if track.phase_inverted == inverted {
        return true;
    }
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_PHASE, inverted, 0) {
        return false;
    }
    if record {
        record_undo(
            ops,
            host,
            b"Phase Invert\0",
            &[track_payload(
                HIRARI_ROUTING_EDIT_PHASE,
                track_id,
                !inverted,
                0,
            )],
            &[track_payload(
                HIRARI_ROUTING_EDIT_PHASE,
                track_id,
                inverted,
                0,
            )],
            false,
            false,
        );
    }
    true
}

const MAX_TRACK_DELAY_SAMPLES: u32 = 8192;

fn set_track_delay_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    samples: u32,
    record: bool,
) -> bool {
    if samples > MAX_TRACK_DELAY_SAMPLES {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples) {
        return false;
    }
    if record {
        let mut name = format!("Track Delay {track_id}").into_bytes();
        name.push(0);
        // `perform` replays the redo payload through this module and records
        // with coalescing, matching the previous performAction call.
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_DELAY,
                track_id,
                false,
                track.delay_samples,
            )],
            &[track_payload(
                HIRARI_ROUTING_EDIT_DELAY,
                track_id,
                false,
                samples,
            )],
            true,
            true,
        );
        // The redo replay is allowed to be a no-op; keep the live value
        // explicit so coalescing cannot leave it stale.
        apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples);
    }
    true
}

const MAX_TRACK_VOLUME: f32 = 2.0;

/// Mix edits record even when the value already matches: the previous engine
/// pushed an undo entry per call and control surfaces rely on that cadence.
fn set_track_volume_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    value: f32,
    record: bool,
) -> bool {
    if !value.is_finite() || !(0.0..=MAX_TRACK_VOLUME).contains(&value) {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_mix(host, track_id, HIRARI_ROUTING_EDIT_VOLUME, value) {
        return false;
    }
    if record {
        let mut name = format!("Track Volume {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_mix_payload(
                HIRARI_ROUTING_EDIT_VOLUME,
                track_id,
                track.volume,
            )],
            &[track_mix_payload(
                HIRARI_ROUTING_EDIT_VOLUME,
                track_id,
                value,
            )],
            false,
            false,
        );
    }
    true
}

fn set_track_pan_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    value: f32,
    record: bool,
) -> bool {
    if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_mix(host, track_id, HIRARI_ROUTING_EDIT_PAN, value) {
        return false;
    }
    if record {
        let mut name = format!("Track Pan {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_mix_payload(HIRARI_ROUTING_EDIT_PAN, track_id, track.pan)],
            &[track_mix_payload(HIRARI_ROUTING_EDIT_PAN, track_id, value)],
            false,
            false,
        );
    }
    true
}

fn set_track_mute_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    muted: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_MUTE, muted, 0) {
        return false;
    }
    if record {
        let mut name = format!("Track Mute {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_MUTE,
                track_id,
                track.muted,
                0,
            )],
            &[track_payload(HIRARI_ROUTING_EDIT_MUTE, track_id, muted, 0)],
            false,
            false,
        );
    }
    true
}

fn set_track_solo_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    solo: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_SOLO, solo, 0) {
        return false;
    }
    if record {
        let mut name = format!("Track Solo {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_SOLO,
                track_id,
                track.solo,
                0,
            )],
            &[track_payload(HIRARI_ROUTING_EDIT_SOLO, track_id, solo, 0)],
            false,
            false,
        );
    }
    true
}

/// Applies one recorded payload. Route payloads are already resolved and are
/// applied directly; sidechain payloads re-run the policy so the undo path
/// keeps the same cycle/rollback behavior as the original C++ recursion.
fn apply_payload_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    edit: &HirariRoutingEditPayload,
) {
    match edit.op {
        HIRARI_ROUTING_EDIT_ROUTE | HIRARI_ROUTING_EDIT_ROUTE_GAIN | HIRARI_ROUTING_EDIT_SEND => {
            apply_route_payload(ops, host, edit);
            build_graph(ops, host);
        }
        HIRARI_ROUTING_EDIT_FEEDBACK => {
            apply_feedback(ops, host, edit.source_id, edit.dest_id, edit.gain);
            mark_layout_changed(ops, host);
        }
        HIRARI_ROUTING_EDIT_SIDECHAIN => {
            set_sidechain_with(
                ops,
                host,
                edit.source_id,
                edit.dest_id,
                edit.plugin_index,
                edit.tap_point,
                edit.enabled,
                false,
            );
        }
        HIRARI_ROUTING_EDIT_ARM
        | HIRARI_ROUTING_EDIT_PHASE
        | HIRARI_ROUTING_EDIT_DELAY
        | HIRARI_ROUTING_EDIT_MUTE
        | HIRARI_ROUTING_EDIT_SOLO => {
            apply_track_scalar(host, edit.source_id, edit.op, edit.enabled, edit.samples);
        }
        HIRARI_ROUTING_EDIT_VOLUME | HIRARI_ROUTING_EDIT_PAN => {
            apply_track_mix(host, edit.source_id, edit.op, edit.gain);
        }
        _ => {}
    }
}

fn apply_payloads_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    entries: &[HirariRoutingEditPayload],
) {
    for entry in entries {
        apply_payload_with(ops, host, entry);
    }
}

fn with_ops<F>(call: F) -> bool
where
    F: FnOnce(&HirariRoutingHostOps) -> bool,
{
    match host_ops() {
        Some(ops) => call(&ops),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_route(
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    enabled: bool,
) -> bool {
    with_ops(|ops| set_route_with(ops, host, source_id, dest_id, enabled))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_route_gain(
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    with_ops(|ops| set_route_gain_with(ops, host, source_id, dest_id, gain, enabled))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_send_route(
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    pre_fader: bool,
    enabled: bool,
) -> bool {
    with_ops(|ops| set_send_route_with(ops, host, source_id, dest_id, gain, pre_fader, enabled))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_feedback_route(
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    with_ops(|ops| set_feedback_route_with(ops, host, source_id, dest_id, gain, enabled))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_sidechain_link(
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    plugin_index: u32,
    tap_point: u32,
    enabled: bool,
) -> bool {
    with_ops(|ops| {
        set_sidechain_with(
            ops,
            host,
            source_id,
            dest_id,
            plugin_index,
            tap_point,
            enabled,
            true,
        )
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_armed(
    host: *mut c_void,
    track_id: u32,
    armed: bool,
) -> bool {
    with_ops(|ops| set_track_armed_with(ops, host, track_id, armed, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_phase_invert(
    host: *mut c_void,
    track_id: u32,
    inverted: bool,
) -> bool {
    with_ops(|ops| set_phase_invert_with(ops, host, track_id, inverted, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_delay_samples(
    host: *mut c_void,
    track_id: u32,
    samples: u32,
) -> bool {
    with_ops(|ops| set_track_delay_with(ops, host, track_id, samples, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_volume(
    host: *mut c_void,
    track_id: u32,
    value: f32,
) -> bool {
    with_ops(|ops| set_track_volume_with(ops, host, track_id, value, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_pan(
    host: *mut c_void,
    track_id: u32,
    value: f32,
) -> bool {
    with_ops(|ops| set_track_pan_with(ops, host, track_id, value, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_mute(
    host: *mut c_void,
    track_id: u32,
    muted: bool,
) -> bool {
    with_ops(|ops| set_track_mute_with(ops, host, track_id, muted, true))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_set_track_solo(
    host: *mut c_void,
    track_id: u32,
    solo: bool,
) -> bool {
    with_ops(|ops| set_track_solo_with(ops, host, track_id, solo, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UndoRecord {
        name: String,
        undo: Vec<HirariRoutingEditPayload>,
    }

    /// The routing graph and the sidechain route table are the real Rust
    /// stores, so the host only fakes the C++ pieces: the sidechain manager
    /// registration and the undo recorder. Track-state edits are covered by
    /// the engine-backed tests in lib_tests_routing.rs, because the session
    /// table those paths use is process-global and can't be swapped here.
    struct TestHost {
        routing: *mut c_void,
        routes: *mut c_void,
        gate: *mut c_void,
        // (destination, plugin, source, tap)
        links: Vec<(u32, u32, u32, u32)>,
        undo: Vec<UndoRecord>,
        layouts: u32,
        publishes: u32,
        source_found: bool,
        destination_found: bool,
        source_prepared: bool,
    }

    impl TestHost {
        fn new() -> Self {
            TestHost {
                routing: crate::routing_engine_core::hirari_routing_engine_create(),
                routes: crate::sidechain_route_store::hirari_sidechain_routes_create(),
                gate: crate::sidechain_manager::hirari_sidechain_engine_create(),
                links: Vec::new(),
                undo: Vec::new(),
                layouts: 0,
                publishes: 0,
                source_found: false,
                destination_found: false,
                source_prepared: false,
            }
        }
    }

    impl Drop for TestHost {
        fn drop(&mut self) {
            unsafe {
                crate::routing_engine_core::hirari_routing_engine_destroy(self.routing);
                crate::sidechain_route_store::hirari_sidechain_routes_destroy(self.routes);
                crate::sidechain_manager::hirari_sidechain_engine_destroy(self.gate);
            }
        }
    }

    unsafe extern "C" fn test_routing_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.routing
    }

    unsafe extern "C" fn test_sidechain_routes_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.routes
    }

    unsafe extern "C" fn test_sidechain_context(
        host: *mut c_void,
        _source: u32,
        _dest: u32,
        out: *mut HirariRoutingSidechainContext,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        *out = HirariRoutingSidechainContext {
            block_size: 512,
            sample_rate: 48_000,
            generation: 1,
            source_found: host.source_found,
            destination_found: host.destination_found,
            source_prepared: host.source_prepared,
        };
        true
    }

    unsafe extern "C" fn test_register_sidechain(
        host: *mut c_void,
        dest: u32,
        plugin: u32,
        source: u32,
        _context: *const HirariRoutingSidechainContext,
        tap: u32,
        _silent: bool,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        host.links
            .retain(|link| !(link.0 == dest && link.1 == plugin));
        host.links.push((dest, plugin, source, tap));
        true
    }

    unsafe extern "C" fn test_sidechain_engine_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.gate
    }

    unsafe extern "C" fn test_mark_layout_changed(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.layouts += 1;
    }

    unsafe extern "C" fn test_sync_structural(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.publishes += 1;
    }

    unsafe extern "C" fn test_record_undo(
        host: *mut c_void,
        name: *const c_char,
        undo: *const HirariRoutingEditPayload,
        undo_count: usize,
        redo: *const HirariRoutingEditPayload,
        redo_count: usize,
        perform: bool,
        _coalesce: bool,
    ) {
        let host = &mut *(host as *mut TestHost);
        let name = if name.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(name)
                .to_string_lossy()
                .into_owned()
        };
        let undo = if undo_count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(undo, undo_count).to_vec()
        };
        let redo = if redo_count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(redo, redo_count).to_vec()
        };
        if perform {
            apply_payloads_with(&test_ops(), host as *mut TestHost as *mut c_void, &redo);
        }
        host.undo.push(UndoRecord { name, undo });
    }

    fn test_ops() -> HirariRoutingHostOps {
        HirariRoutingHostOps {
            routing_store: Some(test_routing_store),
            sidechain_routes_store: Some(test_sidechain_routes_store),
            sidechain_engine_store: Some(test_sidechain_engine_store),
            sidechain_context: Some(test_sidechain_context),
            register_sidechain: Some(test_register_sidechain),
            mark_layout_changed: Some(test_mark_layout_changed),
            sync_structural: Some(test_sync_structural),
            record_undo: Some(test_record_undo),
            undo_state: None,
            engine_block_store: None,
        }
    }

    fn host_ptr(host: &TestHost) -> *mut c_void {
        host as *const TestHost as *mut c_void
    }

    /// Reads the live routing graph through the same path the policy uses.
    fn edge(host: &TestHost, source: u32, dest: u32) -> HirariRoutingEdgeState {
        let ops = test_ops();
        query_edge(&ops, host_ptr(host), source, dest).expect("routing store")
    }

    fn route_table(host: &TestHost) -> Vec<HirariRoutingSidechainEntry> {
        let ops = test_ops();
        list_sidechain(&ops, host_ptr(host)).expect("sidechain route store")
    }

    fn seed_connection(host: &TestHost, source: u32, dest: u32, gain: f32, send: bool) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_connection(
                host.routing, source, dest, gain, send, false,
            )
        };
    }

    fn seed_route(host: &TestHost, source: u32, dest: u32, plugin: u32, tap: u32) {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(
                host.routes, source, dest, plugin, tap,
            )
        };
    }

    fn seed_dependency(host: &TestHost, source: u32, dest: u32) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_dependency(
                host.routing, source, dest,
            )
        };
    }

    fn has_dependency(host: &TestHost, source: u32, dest: u32) -> bool {
        unsafe { crate::routing_engine_core::hirari_routing_engine_has_dependency(host.routing, source, dest) }
    }

    fn sidechain(source: u32, dest: u32, plugin: u32, tap: u32) -> HirariRoutingSidechainEntry {
        HirariRoutingSidechainEntry {
            source_id: source,
            dest_id: dest,
            plugin_index: plugin,
            tap_point: tap,
        }
    }

    #[test]
    fn sidechain_rejects_feedback_cycle_without_applying() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        // Existing edge 1 -> 2. Adding 2 -> 1 would close the loop.
        seed_route(&host, 1, 2, 0, 2);
        let host_ptr = host_ptr(&host);
        assert!(!set_sidechain_with(&ops, host_ptr, 2, 1, 0, 2, true, true));
        assert_eq!(route_table(&host), vec![sidechain(1, 2, 0, 2)]);
        assert!(host.links.is_empty());
        assert!(host.undo.is_empty());
    }

    #[test]
    fn sidechain_replacing_source_repoints_the_existing_route() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        seed_route(&host, 1, 3, 0, 2);
        seed_dependency(&host, 1, 3);
        let host_ptr = host_ptr(&host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        assert_eq!(route_table(&host), vec![sidechain(2, 3, 0, 1)]);
        assert_eq!(host.links, vec![(3, 0, 2, 1)]);
        assert!(has_dependency(&host, 2, 3));
        assert!(!has_dependency(&host, 1, 3));
        assert_eq!(host.undo.len(), 1);
        assert_eq!(host.undo[0].name, "Sidechain Connect");
    }

    #[test]
    fn undo_payload_restores_the_previous_sidechain_link() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        seed_route(&host, 1, 3, 0, 2);
        seed_dependency(&host, 1, 3);
        let host_ptr = host_ptr(&host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        let undo = host.undo[0].undo.clone();
        assert_eq!(undo.len(), 1);
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(route_table(&host), vec![sidechain(1, 3, 0, 2)]);
        assert_eq!(host.links, vec![(3, 0, 1, 2)]);
        assert!(has_dependency(&host, 1, 3));
        assert!(!has_dependency(&host, 2, 3));
    }

    #[test]
    fn route_gain_undo_restores_the_previous_gain() {
        let ops = test_ops();
        let host = TestHost::new();
        seed_connection(&host, 1, 2, 0.75, false);
        let host_ptr = host_ptr(&host);
        assert!(set_route_gain_with(&ops, host_ptr, 1, 2, 0.5, true));
        assert_eq!(edge(&host, 1, 2).route_gain, 0.5);
        let undo = host.undo[0].undo.clone();
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(edge(&host, 1, 2).route_gain, 0.75);
    }

    #[test]
    fn feedback_route_rejects_non_finite_and_missing_remove() {
        let ops = test_ops();
        let host = TestHost::new();
        let host_ptr = host_ptr(&host);
        assert!(!set_feedback_route_with(&ops, host_ptr, 1, 2, f32::NAN, true));
        assert!(!set_feedback_route_with(&ops, host_ptr, 1, 2, 1.0, false));
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 1.5, true));
        assert_eq!(edge(&host, 1, 2).feedback_gain, 1.5);
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 0.0, false));
        assert!(edge(&host, 1, 2).feedback_gain < 0.0);
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn route_edit_without_native_recorder_uses_the_rust_history() {
        let host = TestHost::new();
        let host_ptr = host_ptr(&host);
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = test_ops();
        ops.record_undo = None;
        ops.undo_state = Some(fake_undo_state);
        assert!(set_route_with(&ops, host_ptr, 0, 1, true));
        assert!(edge(&host, 0, 1).route_enabled);
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        assert!(!edge(&host, 0, 1).route_enabled);
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
}
