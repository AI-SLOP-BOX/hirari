//! Host-driven routing and track-state edits.
//!
//! The engine owns the live routing graph, the sidechain manager and the
//! per-track control state because those are consumed by the audio callback.
//! This module owns the edit policy instead: it reads the current state
//! through host callbacks, decides whether the edit is legal, asks the host to
//! apply the resolved mutation, and records exactly one undo entry whose
//! payloads replay through this same module.
//!
//! Every callback runs on the control thread. Nothing here is reachable from
//! the audio callback.

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::plugin_edit_host::{register_replay, UndoStateFn};

/// Matches `RoutingEngine::kMaxNodes`; ids at or above it are not routable.
const MAX_ROUTING_NODES: u32 = 128;
/// Maximum sidechain routes mirrored for one policy decision. Rejecting the
/// edit when the table is larger keeps cycle detection complete instead of
/// silently truncating it.
const MAX_SIDECHAIN_ROUTES: usize = 512;

/// Payload ops. The numeric values are shared with
/// `rust_ffi.hpp::HirariRoutingEditOp`.
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
#[derive(Clone, Copy, Debug, Default)]
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

pub type QueryEdgeFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut HirariRoutingEdgeState) -> bool;
pub type QueryTrackFn =
    unsafe extern "C" fn(*mut c_void, u32, *mut HirariRoutingTrackState) -> bool;
pub type ListSidechainFn =
    unsafe extern "C" fn(*mut c_void, *mut HirariRoutingSidechainEntry, usize) -> usize;
pub type SidechainContextFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut HirariRoutingSidechainContext) -> bool;
pub type ApplyRouteFn = unsafe extern "C" fn(*mut c_void, *const HirariRoutingEditPayload) -> bool;
pub type ApplyFeedbackFn = unsafe extern "C" fn(*mut c_void, u32, u32, f32) -> bool;
pub type ApplyTrackScalarFn = unsafe extern "C" fn(*mut c_void, u32, i32, bool, u32) -> bool;
pub type ApplyTrackMixFn = unsafe extern "C" fn(*mut c_void, u32, i32, f32) -> bool;
pub type RemoveSidechainFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> bool;
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
pub type WriteSidechainRouteFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> bool;
pub type DependencyFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> bool;
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
    pub query_edge: Option<QueryEdgeFn>,
    pub query_track: Option<QueryTrackFn>,
    pub list_sidechain: Option<ListSidechainFn>,
    pub sidechain_context: Option<SidechainContextFn>,
    pub apply_route: Option<ApplyRouteFn>,
    pub apply_feedback: Option<ApplyFeedbackFn>,
    pub apply_track_scalar: Option<ApplyTrackScalarFn>,
    pub remove_sidechain: Option<RemoveSidechainFn>,
    pub register_sidechain: Option<RegisterSidechainFn>,
    pub write_sidechain_route: Option<WriteSidechainRouteFn>,
    pub add_dependency: Option<DependencyFn>,
    pub remove_dependency: Option<DependencyFn>,
    pub build_graph: Option<HostFn>,
    pub mark_layout_changed: Option<HostFn>,
    pub publish_routes: Option<HostFn>,
    pub record_undo: Option<RecordUndoFn>,
    pub apply_track_mix: Option<ApplyTrackMixFn>,
    pub undo_state: Option<UndoStateFn>,
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

fn payload(op: i32, source_id: u32, dest_id: u32) -> HirariRoutingEditPayload {
    HirariRoutingEditPayload {
        op,
        source_id,
        dest_id,
        ..Default::default()
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

fn query_edge(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingEdgeState> {
    let query = ops.query_edge?;
    let mut state = HirariRoutingEdgeState::default();
    unsafe { query(host, source_id, dest_id, &mut state) }.then_some(state)
}

fn query_track(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
) -> Option<HirariRoutingTrackState> {
    let query = ops.query_track?;
    let mut state = HirariRoutingTrackState::default();
    unsafe { query(host, track_id, &mut state) }.then_some(state)
}

fn list_sidechain(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
) -> Option<Vec<HirariRoutingSidechainEntry>> {
    let list = ops.list_sidechain?;
    let mut buffer = vec![HirariRoutingSidechainEntry::default(); MAX_SIDECHAIN_ROUTES];
    let total = unsafe { list(host, buffer.as_mut_ptr(), buffer.len()) };
    if total > buffer.len() {
        return None;
    }
    buffer.truncate(total);
    Some(buffer)
}

fn sidechain_context(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingSidechainContext> {
    let query = ops.sidechain_context?;
    let mut context = HirariRoutingSidechainContext::default();
    unsafe { query(host, source_id, dest_id, &mut context) }.then_some(context)
}

fn build_graph(ops: &HirariRoutingHostOps, host: *mut c_void) -> bool {
    let Some(build) = ops.build_graph else {
        return false;
    };
    unsafe { build(host) };
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
    match ops.add_dependency {
        Some(add) => unsafe { add(host, source_id, dest_id) },
        None => false,
    }
}

fn remove_dependency(ops: &HirariRoutingHostOps, host: *mut c_void, source_id: u32, dest_id: u32) {
    if let Some(remove) = ops.remove_dependency {
        unsafe { remove(host, source_id, dest_id) };
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
    let Some(apply) = ops.apply_route else {
        return false;
    };
    if !unsafe {
        apply(
            host,
            &route_payload(
                HIRARI_ROUTING_EDIT_ROUTE,
                source_id,
                dest_id,
                1.0,
                enabled,
                false,
            ),
        )
    } {
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
    if let Some(publish) = ops.publish_routes {
        unsafe { publish(host) };
    }
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
    let Some(apply) = ops.apply_route else {
        return false;
    };
    if !unsafe {
        apply(
            host,
            &route_payload(
                HIRARI_ROUTING_EDIT_ROUTE_GAIN,
                source_id,
                dest_id,
                gain,
                enabled,
                false,
            ),
        )
    } {
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
    if let Some(publish) = ops.publish_routes {
        unsafe { publish(host) };
    }
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
        if query_track(ops, host, source_id).is_none() {
            return false;
        }
        let Some(destination) = query_track(ops, host, dest_id) else {
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
    let Some(apply) = ops.apply_route else {
        return false;
    };
    if !unsafe {
        apply(
            host,
            &route_payload(
                HIRARI_ROUTING_EDIT_SEND,
                source_id,
                dest_id,
                gain,
                enabled,
                pre_fader,
            ),
        )
    } {
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
    if let Some(publish) = ops.publish_routes {
        unsafe { publish(host) };
    }
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
    let Some(apply) = ops.apply_feedback else {
        return false;
    };
    if enabled {
        if !unsafe { apply(host, source_id, dest_id, gain) } {
            return false;
        }
    } else {
        if before.feedback_gain < 0.0 {
            return false;
        }
        unsafe { apply(host, source_id, dest_id, -1.0) };
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
        let Some(remove) = ops.remove_sidechain else {
            return false;
        };
        let removed = unsafe { remove(host, dest_id, plugin_index) };
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
    let Some(register) = ops.register_sidechain else {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    };
    let silent = !context.source_prepared;
    if !unsafe {
        register(
            host,
            dest_id,
            plugin_index,
            source_id,
            &context,
            tap_point,
            silent,
        )
    } {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    }
    if let Some(write) = ops.write_sidechain_route {
        unsafe { write(host, source_id, dest_id, plugin_index, tap_point) };
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    // An already-correct value is a valid idempotent application.
    if track.armed == armed {
        return true;
    }
    let Some(apply) = ops.apply_track_scalar else {
        return false;
    };
    if !unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_ARM, armed, 0) } {
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    if track.phase_inverted == inverted {
        return true;
    }
    let Some(apply) = ops.apply_track_scalar else {
        return false;
    };
    if !unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_PHASE, inverted, 0) } {
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    let Some(apply) = ops.apply_track_scalar else {
        return false;
    };
    if !unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples) } {
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
        unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples) };
    }
    true
}

const MAX_TRACK_VOLUME: f32 = 2.0;

fn apply_track_mix(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    op: i32,
    value: f32,
) -> bool {
    match ops.apply_track_mix {
        Some(apply) => unsafe { apply(host, track_id, op, value) },
        None => false,
    }
}

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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    if !apply_track_mix(ops, host, track_id, HIRARI_ROUTING_EDIT_VOLUME, value) {
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    if !apply_track_mix(ops, host, track_id, HIRARI_ROUTING_EDIT_PAN, value) {
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    let Some(apply) = ops.apply_track_scalar else {
        return false;
    };
    if !unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_MUTE, muted, 0) } {
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
    let Some(track) = query_track(ops, host, track_id) else {
        return false;
    };
    let Some(apply) = ops.apply_track_scalar else {
        return false;
    };
    if !unsafe { apply(host, track_id, HIRARI_ROUTING_EDIT_SOLO, solo, 0) } {
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
        HIRARI_ROUTING_EDIT_ROUTE | HIRARI_ROUTING_EDIT_ROUTE_GAIN => {
            if let Some(apply) = ops.apply_route {
                unsafe { apply(host, edit) };
            }
            build_graph(ops, host);
        }
        HIRARI_ROUTING_EDIT_SEND => {
            if let Some(apply) = ops.apply_route {
                unsafe { apply(host, edit) };
            }
            build_graph(ops, host);
        }
        HIRARI_ROUTING_EDIT_FEEDBACK => {
            if let Some(apply) = ops.apply_feedback {
                unsafe { apply(host, edit.source_id, edit.dest_id, edit.gain) };
            }
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
            if let Some(apply) = ops.apply_track_scalar {
                unsafe { apply(host, edit.source_id, edit.op, edit.enabled, edit.samples) };
            }
        }
        HIRARI_ROUTING_EDIT_VOLUME | HIRARI_ROUTING_EDIT_PAN => {
            if let Some(apply) = ops.apply_track_mix {
                unsafe { apply(host, edit.source_id, edit.op, edit.gain) };
            }
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

/// Replays a recorded undo or redo payload list through the same policy.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_edit_apply_payloads(
    host: *mut c_void,
    entries: *const HirariRoutingEditPayload,
    count: usize,
) {
    let Some(ops) = host_ops() else {
        return;
    };
    if entries.is_null() || count == 0 {
        return;
    }
    let payloads = unsafe { std::slice::from_raw_parts(entries, count) };
    apply_payloads_with(&ops, host, payloads);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Default)]
    struct TestTrack {
        armed: bool,
        phase: bool,
        delay: u32,
        is_bus: bool,
        volume: f32,
        pan: f32,
        muted: bool,
        solo: bool,
    }

    #[derive(Clone, Copy)]
    struct TestConnection {
        source: u32,
        dest: u32,
        gain: f32,
        send: bool,
        pre_fader: bool,
    }

    struct UndoRecord {
        name: String,
        undo: Vec<HirariRoutingEditPayload>,
        redo: Vec<HirariRoutingEditPayload>,
        perform: bool,
        coalesce: bool,
    }

    #[derive(Default)]
    struct TestHost {
        tracks: Vec<(u32, TestTrack)>,
        connections: Vec<TestConnection>,
        feedback: Vec<(u32, u32, f32)>,
        routes: Vec<HirariRoutingSidechainEntry>,
        // (destination, plugin, source, tap)
        links: Vec<(u32, u32, u32, u32)>,
        dependencies: Vec<(u32, u32)>,
        undo: Vec<UndoRecord>,
        graphs: u32,
        layouts: u32,
        publishes: u32,
        source_found: bool,
        destination_found: bool,
        source_prepared: bool,
    }

    unsafe extern "C" fn test_query_edge(
        host: *mut c_void,
        source: u32,
        dest: u32,
        out: *mut HirariRoutingEdgeState,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        let mut state = HirariRoutingEdgeState::default();
        if let Some(conn) = host
            .connections
            .iter()
            .find(|conn| conn.source == source && conn.dest == dest && !conn.send)
        {
            state.route_enabled = true;
            state.route_gain = conn.gain;
        }
        if let Some(conn) = host
            .connections
            .iter()
            .find(|conn| conn.source == source && conn.dest == dest && conn.send)
        {
            state.send_gain = conn.gain.max(0.0);
            state.send_pre_fader = conn.pre_fader;
        }
        state.feedback_gain = host
            .feedback
            .iter()
            .find(|edge| edge.0 == source && edge.1 == dest)
            .map_or(-1.0, |edge| edge.2);
        *out = state;
        true
    }

    unsafe extern "C" fn test_query_track(
        host: *mut c_void,
        track_id: u32,
        out: *mut HirariRoutingTrackState,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        let Some((_, track)) = host.tracks.iter().find(|(id, _)| *id == track_id) else {
            return false;
        };
        *out = HirariRoutingTrackState {
            armed: track.armed,
            phase_inverted: track.phase,
            is_bus: track.is_bus,
            delay_samples: track.delay,
            volume: track.volume,
            pan: track.pan,
            muted: track.muted,
            solo: track.solo,
        };
        true
    }

    unsafe extern "C" fn test_list_sidechain(
        host: *mut c_void,
        out: *mut HirariRoutingSidechainEntry,
        capacity: usize,
    ) -> usize {
        let host = &mut *(host as *mut TestHost);
        for (index, route) in host.routes.iter().enumerate() {
            if index < capacity {
                *out.add(index) = *route;
            }
        }
        host.routes.len()
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

    unsafe extern "C" fn test_apply_route(
        host: *mut c_void,
        payload: *const HirariRoutingEditPayload,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        let edit = &*payload;
        let send = edit.op == HIRARI_ROUTING_EDIT_SEND;
        host.connections.retain(|conn| {
            !(conn.source == edit.source_id && conn.dest == edit.dest_id && conn.send == send)
        });
        if edit.enabled {
            host.connections.push(TestConnection {
                source: edit.source_id,
                dest: edit.dest_id,
                gain: edit.gain,
                send,
                pre_fader: edit.pre_fader,
            });
        }
        true
    }

    unsafe extern "C" fn test_apply_feedback(
        host: *mut c_void,
        source: u32,
        dest: u32,
        gain: f32,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        host.feedback
            .retain(|edge| !(edge.0 == source && edge.1 == dest));
        if gain >= 0.0 {
            host.feedback.push((source, dest, gain));
        }
        true
    }

    unsafe extern "C" fn test_apply_track_scalar(
        host: *mut c_void,
        track_id: u32,
        op: i32,
        enabled: bool,
        samples: u32,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        let Some((_, track)) = host.tracks.iter_mut().find(|(id, _)| *id == track_id) else {
            return false;
        };
        match op {
            HIRARI_ROUTING_EDIT_ARM => track.armed = enabled,
            HIRARI_ROUTING_EDIT_PHASE => track.phase = enabled,
            HIRARI_ROUTING_EDIT_DELAY => track.delay = samples,
            HIRARI_ROUTING_EDIT_MUTE => track.muted = enabled,
            HIRARI_ROUTING_EDIT_SOLO => track.solo = enabled,
            _ => return false,
        }
        true
    }

    unsafe extern "C" fn test_apply_track_mix(
        host: *mut c_void,
        track_id: u32,
        op: i32,
        value: f32,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        let Some((_, track)) = host.tracks.iter_mut().find(|(id, _)| *id == track_id) else {
            return false;
        };
        match op {
            HIRARI_ROUTING_EDIT_VOLUME => track.volume = value,
            HIRARI_ROUTING_EDIT_PAN => track.pan = value,
            _ => return false,
        }
        true
    }

    unsafe extern "C" fn test_remove_sidechain(host: *mut c_void, dest: u32, plugin: u32) -> bool {
        let host = &mut *(host as *mut TestHost);
        let route_count = host.routes.len();
        host.routes
            .retain(|route| !(route.dest_id == dest && route.plugin_index == plugin));
        let link_count = host.links.len();
        host.links
            .retain(|link| !(link.0 == dest && link.1 == plugin));
        route_count != host.routes.len() || link_count != host.links.len()
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

    unsafe extern "C" fn test_write_sidechain_route(
        host: *mut c_void,
        source: u32,
        dest: u32,
        plugin: u32,
        tap: u32,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        if let Some(route) = host
            .routes
            .iter_mut()
            .find(|route| route.dest_id == dest && route.plugin_index == plugin)
        {
            route.source_id = source;
            route.tap_point = tap;
        } else {
            host.routes.push(HirariRoutingSidechainEntry {
                source_id: source,
                dest_id: dest,
                plugin_index: plugin,
                tap_point: tap,
            });
        }
        true
    }

    unsafe extern "C" fn test_add_dependency(host: *mut c_void, source: u32, dest: u32) -> bool {
        let host = &mut *(host as *mut TestHost);
        if !host.dependencies.contains(&(source, dest)) {
            host.dependencies.push((source, dest));
        }
        true
    }

    unsafe extern "C" fn test_remove_dependency(host: *mut c_void, source: u32, dest: u32) -> bool {
        let host = &mut *(host as *mut TestHost);
        host.dependencies.retain(|edge| *edge != (source, dest));
        true
    }

    unsafe extern "C" fn test_build_graph(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.graphs += 1;
    }

    unsafe extern "C" fn test_mark_layout_changed(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.layouts += 1;
    }

    unsafe extern "C" fn test_publish_routes(host: *mut c_void) {
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
        coalesce: bool,
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
        host.undo.push(UndoRecord {
            name,
            undo,
            redo,
            perform,
            coalesce,
        });
    }

    fn test_ops() -> HirariRoutingHostOps {
        HirariRoutingHostOps {
            query_edge: Some(test_query_edge),
            query_track: Some(test_query_track),
            list_sidechain: Some(test_list_sidechain),
            sidechain_context: Some(test_sidechain_context),
            apply_route: Some(test_apply_route),
            apply_feedback: Some(test_apply_feedback),
            apply_track_scalar: Some(test_apply_track_scalar),
            remove_sidechain: Some(test_remove_sidechain),
            register_sidechain: Some(test_register_sidechain),
            write_sidechain_route: Some(test_write_sidechain_route),
            add_dependency: Some(test_add_dependency),
            remove_dependency: Some(test_remove_dependency),
            build_graph: Some(test_build_graph),
            mark_layout_changed: Some(test_mark_layout_changed),
            publish_routes: Some(test_publish_routes),
            record_undo: Some(test_record_undo),
            apply_track_mix: Some(test_apply_track_mix),
            undo_state: None,
        }
    }

    fn host_ptr(host: &mut TestHost) -> *mut c_void {
        host as *mut TestHost as *mut c_void
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
        let mut host = TestHost::default();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        // Existing edge 1 -> 2. Adding 2 -> 1 would close the loop.
        host.routes.push(sidechain(1, 2, 0, 2));
        let host_ptr = host_ptr(&mut host);
        assert!(!set_sidechain_with(&ops, host_ptr, 2, 1, 0, 2, true, true));
        assert_eq!(host.routes.len(), 1);
        assert_eq!(host.routes[0].source_id, 1);
        assert!(host.links.is_empty());
        assert!(host.undo.is_empty());
    }

    #[test]
    fn sidechain_replacing_source_repoints_the_existing_route() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        host.routes.push(sidechain(1, 3, 0, 2));
        host.dependencies.push((1, 3));
        let host_ptr = host_ptr(&mut host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        assert_eq!(host.routes.len(), 1);
        assert_eq!(host.routes[0].source_id, 2);
        assert_eq!(host.routes[0].tap_point, 1);
        assert_eq!(host.links, vec![(3, 0, 2, 1)]);
        assert!(host.dependencies.contains(&(2, 3)));
        assert!(!host.dependencies.contains(&(1, 3)));
        assert_eq!(host.undo.len(), 1);
        assert_eq!(host.undo[0].name, "Sidechain Connect");
    }

    #[test]
    fn arm_and_phase_are_idempotent_and_record_only_real_changes() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.tracks.push((7, TestTrack::default()));
        let host_ptr = host_ptr(&mut host);
        // Already disarmed: success without an undo entry.
        assert!(set_track_armed_with(&ops, host_ptr, 7, false, true));
        assert!(host.undo.is_empty());
        assert!(set_track_armed_with(&ops, host_ptr, 7, true, true));
        assert_eq!(host.undo.len(), 1);
        assert_eq!(host.undo[0].name, "Record Arm");
        assert!(set_track_armed_with(&ops, host_ptr, 7, true, true));
        assert_eq!(host.undo.len(), 1);
        // Same for phase inversion.
        assert!(set_phase_invert_with(&ops, host_ptr, 7, false, true));
        assert_eq!(host.undo.len(), 1);
        assert!(set_phase_invert_with(&ops, host_ptr, 7, true, true));
        assert_eq!(host.undo.len(), 2);
        assert_eq!(host.undo[1].name, "Phase Invert");
    }

    #[test]
    fn undo_payload_restores_the_previous_sidechain_link() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        host.routes.push(sidechain(1, 3, 0, 2));
        host.dependencies.push((1, 3));
        let host_ptr = host_ptr(&mut host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        let undo = host.undo[0].undo.clone();
        assert_eq!(undo.len(), 1);
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(host.routes.len(), 1);
        assert_eq!(host.routes[0].source_id, 1);
        assert_eq!(host.routes[0].tap_point, 2);
        assert_eq!(host.links, vec![(3, 0, 1, 2)]);
        assert!(host.dependencies.contains(&(1, 3)));
        assert!(!host.dependencies.contains(&(2, 3)));
    }

    #[test]
    fn route_gain_undo_restores_the_previous_gain() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.connections.push(TestConnection {
            source: 1,
            dest: 2,
            gain: 0.75,
            send: false,
            pre_fader: false,
        });
        let host_ptr = host_ptr(&mut host);
        assert!(set_route_gain_with(&ops, host_ptr, 1, 2, 0.5, true));
        assert_eq!(host.connections[0].gain, 0.5);
        let undo = host.undo[0].undo.clone();
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(host.connections[0].gain, 0.75);
    }

    #[test]
    fn send_route_requires_bus_destination_and_existing_tracks() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.tracks.push((1, TestTrack::default()));
        host.tracks.push((
            2,
            TestTrack {
                is_bus: false,
                ..Default::default()
            },
        ));
        let host_ptr = host_ptr(&mut host);
        assert!(!set_send_route_with(&ops, host_ptr, 1, 2, 1.0, false, true));
        host.tracks[1].1.is_bus = true;
        assert!(set_send_route_with(&ops, host_ptr, 1, 2, 1.0, false, true));
        assert_eq!(host.undo.len(), 1);
        assert_eq!(host.undo[0].name, "Audio Send");
        // Missing source is rejected even after the destination becomes valid.
        assert!(!set_send_route_with(&ops, host_ptr, 9, 2, 1.0, false, true));
    }

    #[test]
    fn feedback_route_rejects_non_finite_and_missing_remove() {
        let ops = test_ops();
        let mut host = TestHost::default();
        let host_ptr = host_ptr(&mut host);
        assert!(!set_feedback_route_with(
            &ops,
            host_ptr,
            1,
            2,
            f32::NAN,
            true
        ));
        assert!(!set_feedback_route_with(&ops, host_ptr, 1, 2, 1.0, false));
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 1.5, true));
        assert_eq!(host.feedback, vec![(1, 2, 1.5)]);
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 0.0, false));
        assert!(host.feedback.is_empty());
    }

    #[test]
    fn mix_edits_apply_and_restore_the_previous_value() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.tracks.push((
            3,
            TestTrack {
                volume: 1.0,
                pan: -0.25,
                ..Default::default()
            },
        ));
        let host_ptr = host_ptr(&mut host);
        assert!(set_track_volume_with(&ops, host_ptr, 3, 0.5, true));
        assert!(set_track_pan_with(&ops, host_ptr, 3, 0.75, true));
        assert_eq!(host.tracks[0].1.volume, 0.5);
        assert_eq!(host.tracks[0].1.pan, 0.75);
        assert_eq!(host.undo.len(), 2);
        assert_eq!(host.undo[0].name, "Track Volume 3");
        assert_eq!(host.undo[1].name, "Track Pan 3");
        assert_eq!(host.undo[0].undo[0].gain, 1.0);
        assert_eq!(host.undo[1].undo[0].gain, -0.25);
        let volume_undo = host.undo[0].undo.clone();
        let pan_undo = host.undo[1].undo.clone();
        apply_payloads_with(&ops, host_ptr, &volume_undo);
        apply_payloads_with(&ops, host_ptr, &pan_undo);
        assert_eq!(host.tracks[0].1.volume, 1.0);
        assert_eq!(host.tracks[0].1.pan, -0.25);
    }

    #[test]
    fn mix_ranges_reject_out_of_domain_values() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.tracks.push((
            4,
            TestTrack {
                volume: 1.0,
                ..Default::default()
            },
        ));
        let host_ptr = host_ptr(&mut host);
        assert!(!set_track_volume_with(&ops, host_ptr, 4, 2.5, true));
        assert!(!set_track_volume_with(&ops, host_ptr, 4, -0.1, true));
        assert!(!set_track_volume_with(&ops, host_ptr, 4, f32::NAN, true));
        assert!(set_track_volume_with(&ops, host_ptr, 4, 2.0, true));
        assert!(!set_track_pan_with(&ops, host_ptr, 4, 1.5, true));
        assert!(!set_track_pan_with(&ops, host_ptr, 4, f32::INFINITY, true));
        assert!(set_track_pan_with(&ops, host_ptr, 4, -1.0, true));
        assert!(!set_track_mute_with(&ops, host_ptr, 9, true, true));
        assert_eq!(host.undo.len(), 2);
    }

    #[test]
    fn mute_and_solo_record_unchanged_values_like_the_previous_engine() {
        let ops = test_ops();
        let mut host = TestHost::default();
        host.tracks.push((5, TestTrack::default()));
        let host_ptr = host_ptr(&mut host);
        assert!(set_track_mute_with(&ops, host_ptr, 5, false, true));
        assert!(set_track_solo_with(&ops, host_ptr, 5, true, true));
        assert!(set_track_solo_with(&ops, host_ptr, 5, true, true));
        assert_eq!(host.undo.len(), 3);
        assert_eq!(host.undo[0].name, "Track Mute 5");
        assert_eq!(host.undo[1].name, "Track Solo 5");
        assert!(!host.undo[0].undo[0].enabled);
        assert!(!host.undo[1].undo[0].enabled);
        assert!(host.tracks[0].1.solo);
        let undo = host.undo[1].undo.clone();
        apply_payloads_with(&ops, host_ptr, &undo);
        assert!(!host.tracks[0].1.solo);
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn route_edit_without_native_recorder_uses_the_rust_history() {
        let mut host = TestHost::default();
        let host_ptr = host_ptr(&mut host);
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = test_ops();
        ops.record_undo = None;
        ops.undo_state = Some(fake_undo_state);
        assert!(set_route_with(&ops, host_ptr, 0, 1, true));
        assert_eq!(host.connections.len(), 1);
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        assert!(host.connections.is_empty());
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
}
