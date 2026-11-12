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

#[path = "routing_query.rs"]
mod query;
pub(in crate::routing_edit_host) use query::*;

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

#[path = "routing_set_with.rs"]
mod set_with;
pub(in crate::routing_edit_host) use set_with::*;

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
#[path = "routing_edit_host_tests.rs"]
mod tests;
