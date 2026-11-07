//! Composition root for the engine's routing state. The graph, gain, feedback,
//! send-PDC and topology stores used to be five fields of a C++ class that did
//! nothing but forward each call; this module owns them as one object, so the
//! host keeps a single opaque pointer and every routing operation has exactly
//! one implementation.

use std::ffi::{c_char, c_void};

use crate::routing_feedback::FeedbackConnection;
use crate::routing_graph_pdc::RoutingConnectionRecord;

const MAX_NODES: u32 = 128;
const MAX_FEEDBACK_CONNECTIONS: usize = 16;

struct RoutingEngineCore {
    graph: *mut c_void,
    gains: *mut c_void,
    feedback: *mut c_void,
    send_pdc: *mut c_void,
    topology: *mut c_void,
}

impl Drop for RoutingEngineCore {
    fn drop(&mut self) {
        unsafe {
            crate::routing_graph_pdc::hirari_routing_graph_destroy(self.graph);
            crate::routing_gains::hirari_routing_gains_destroy(self.gains);
            crate::routing_feedback::hirari_routing_feedback_destroy(self.feedback);
            crate::send_pdc_manager::hirari_send_pdc_manager_destroy(self.send_pdc);
            crate::routing_topology::hirari_routing_topology_destroy(self.topology);
        }
    }
}

unsafe fn engine<'a>(state: *const c_void) -> Option<&'a RoutingEngineCore> {
    unsafe { state.cast::<RoutingEngineCore>().as_ref() }
}

#[no_mangle]
pub extern "C" fn hirari_routing_engine_create() -> *mut c_void {
    let core = RoutingEngineCore {
        graph: crate::routing_graph_pdc::hirari_routing_graph_create(),
        gains: crate::routing_gains::hirari_routing_gains_create(),
        feedback: crate::routing_feedback::hirari_routing_feedback_create(),
        send_pdc: crate::send_pdc_manager::hirari_send_pdc_manager_create(),
        topology: crate::routing_topology::hirari_routing_topology_create(),
    };
    Box::into_raw(Box::new(core)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state.cast::<RoutingEngineCore>()) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_reset(state: *mut c_void) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_reset(
            core.graph,
            core.gains,
            core.feedback,
            core.send_pdc,
            core.topology,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_add_connection(
    state: *mut c_void,
    source: u32,
    destination: u32,
    gain: f32,
    send: bool,
    pre_fader: bool,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_add_connection(
            core.graph,
            core.gains,
            core.topology,
            source,
            destination,
            gain,
            send,
            pre_fader,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_connection(
    state: *mut c_void,
    source: u32,
    destination: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_remove_connection(
            core.graph,
            core.gains,
            core.topology,
            source,
            destination,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_send(
    state: *mut c_void,
    source: u32,
    destination: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_remove_send(
            core.graph,
            core.gains,
            core.send_pdc,
            core.topology,
            source,
            destination,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_add_dependency(
    state: *mut c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_add_dependency(
            core.graph,
            core.topology,
            source,
            destination,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_dependency(
    state: *mut c_void,
    source: u32,
    destination: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_remove_dependency(
            core.graph,
            core.topology,
            source,
            destination,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_has_dependency(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_has_dependency(
            core.graph,
            source,
            destination,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_dependencies_for_node(
    state: *mut c_void,
    node: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_runtime::hirari_routing_runtime_remove_dependencies_for_node(
            core.graph,
            core.topology,
            node,
        );
    }
}

/// Moves a snapshot's connection records into a buffer this side owns.
unsafe fn take_connection_snapshot(
    snapshot: *mut c_void,
    count: *mut usize,
) -> *mut RoutingConnectionRecord {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    if snapshot.is_null() {
        return std::ptr::null_mut();
    }
    let records = unsafe { &*snapshot.cast::<Vec<RoutingConnectionRecord>>() };
    let owned = records.clone().into_boxed_slice();
    unsafe { crate::routing_graph_pdc::hirari_routing_graph_snapshot_destroy(snapshot) };
    let len = owned.len();
    if len == 0 {
        return std::ptr::null_mut();
    }
    if let Some(count) = unsafe { count.as_mut() } {
        *count = len;
    }
    Box::into_raw(owned).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_connections_for_node(
    state: *mut c_void,
    node: u32,
    count: *mut usize,
) -> *mut RoutingConnectionRecord {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    if node >= MAX_NODES {
        return std::ptr::null_mut();
    }
    let Some(core) = (unsafe { engine(state) }) else {
        return std::ptr::null_mut();
    };
    let snapshot = unsafe {
        crate::routing_runtime::hirari_routing_runtime_remove_connections_for_node(
            core.graph,
            core.gains,
            core.send_pdc,
            core.topology,
            node,
        )
    };
    unsafe { take_connection_snapshot(snapshot, count) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_copy_connections(
    state: *const c_void,
    count: *mut usize,
) -> *mut RoutingConnectionRecord {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    let Some(core) = (unsafe { engine(state) }) else {
        return std::ptr::null_mut();
    };
    let snapshot = unsafe { crate::routing_graph_pdc::hirari_routing_graph_snapshot_create(core.graph) };
    unsafe { take_connection_snapshot(snapshot, count) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_connection_array_free(
    records: *mut RoutingConnectionRecord,
    count: usize,
) {
    if records.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(records, count);
    drop(unsafe { Box::from_raw(slice) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_feedback_for_node(
    state: *mut c_void,
    node: u32,
    count: *mut usize,
) -> *mut FeedbackConnection {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    if node >= MAX_NODES {
        return std::ptr::null_mut();
    }
    let Some(core) = (unsafe { engine(state) }) else {
        return std::ptr::null_mut();
    };
    let mut buffer = [FeedbackConnection::default(); MAX_FEEDBACK_CONNECTIONS];
    let removed = unsafe {
        crate::routing_feedback::hirari_routing_feedback_remove_node(
            core.feedback,
            node,
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    }
    .min(buffer.len());
    let owned = buffer[..removed].to_vec().into_boxed_slice();
    let len = owned.len();
    if len == 0 {
        return std::ptr::null_mut();
    }
    if let Some(count) = unsafe { count.as_mut() } {
        *count = len;
    }
    Box::into_raw(owned).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_copy_feedback(
    state: *const c_void,
    count: *mut usize,
) -> *mut FeedbackConnection {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    let Some(core) = (unsafe { engine(state) }) else {
        return std::ptr::null_mut();
    };
    let mut buffer = [FeedbackConnection::default(); MAX_FEEDBACK_CONNECTIONS];
    let copied = unsafe {
        crate::routing_feedback::hirari_routing_feedback_copy(
            core.feedback,
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    }
    .min(buffer.len());
    let owned = buffer[..copied].to_vec().into_boxed_slice();
    let len = owned.len();
    if len == 0 {
        return std::ptr::null_mut();
    }
    if let Some(count) = unsafe { count.as_mut() } {
        *count = len;
    }
    Box::into_raw(owned).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_array_free(
    records: *mut FeedbackConnection,
    count: usize,
) {
    if records.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(records, count);
    drop(unsafe { Box::from_raw(slice) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_has_connection(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe { crate::routing_gains::hirari_routing_gains_route(core.gains, source, destination) > 0.0 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_connection_gain(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    let Some(core) = (unsafe { engine(state) }) else {
        return 0.0;
    };
    unsafe { crate::routing_gains::hirari_routing_gains_route(core.gains, source, destination) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_send_gain(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    let Some(core) = (unsafe { engine(state) }) else {
        return 0.0;
    };
    unsafe { crate::routing_gains::hirari_routing_gains_send(core.gains, source, destination) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_send_pre_fader(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe { crate::routing_gains::hirari_routing_gains_send_pre_fader(core.gains, source, destination) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_feedback_gain(
    state: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    let Some(core) = (unsafe { engine(state) }) else {
        return -1.0;
    };
    unsafe { crate::routing_feedback::hirari_routing_feedback_gain(core.feedback, source, destination) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_should_process(
    state: *const c_void,
    source: u32,
    offline_target_active: bool,
    offline_target: u32,
    any_solo: bool,
    source_solo: bool,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_gains::hirari_routing_should_process_node(
            core.gains,
            source,
            offline_target_active,
            offline_target,
            any_solo,
            source_solo,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_add_feedback(
    state: *mut c_void,
    source: u32,
    destination: u32,
    gain: f32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_feedback::hirari_routing_feedback_add(core.feedback, source, destination, gain)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_remove_feedback(
    state: *mut c_void,
    source: u32,
    destination: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_feedback::hirari_routing_feedback_remove(core.feedback, source, destination);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_build(state: *mut c_void) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe { crate::routing_topology::hirari_routing_topology_build(core.topology, core.graph) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_inject_feedback(
    state: *const c_void,
    destination: u32,
    left: *mut f32,
    right: *mut f32,
    len: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_feedback::hirari_routing_feedback_inject(
            core.feedback,
            destination,
            left,
            right,
            len,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_capture_feedback(
    state: *mut c_void,
    source: u32,
    left: *const f32,
    right: *const f32,
    len: u32,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_feedback::hirari_routing_feedback_capture(
            core.feedback,
            source,
            left,
            right,
            len,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_connections_json(
    state: *const c_void,
) -> *mut c_char {
    let Some(core) = (unsafe { engine(state) }) else {
        return std::ptr::null_mut();
    };
    unsafe { crate::routing_graph_pdc::hirari_routing_graph_snapshot_json(core.graph) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_connections_json_free(json: *mut c_char) {
    if json.is_null() {
        return;
    }
    unsafe { crate::routing_graph_pdc::hirari_routing_graph_json_free(json) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_set_send_pdc(
    state: *mut c_void,
    source: u32,
    destination: u32,
    samples: u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::send_pdc_manager::hirari_send_pdc_manager_set_delay(
            core.send_pdc,
            source,
            destination,
            samples,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_enter_send_pdc_read(state: *mut c_void) {
    if let Some(core) = unsafe { engine(state) } {
        unsafe { crate::send_pdc_manager::hirari_send_pdc_manager_enter_read(core.send_pdc) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_leave_send_pdc_read(state: *mut c_void) {
    if let Some(core) = unsafe { engine(state) } {
        unsafe { crate::send_pdc_manager::hirari_send_pdc_manager_leave_read(core.send_pdc) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_resolve_process_order(
    state: *const c_void,
    active_ids: *const u32,
    active_count: usize,
    output: *mut u32,
    output_capacity: usize,
    count_out: *mut u32,
) -> bool {
    let Some(core) = (unsafe { engine(state) }) else {
        return false;
    };
    unsafe {
        crate::routing_topology::hirari_routing_topology_resolve_process_order(
            core.topology,
            active_ids,
            active_count,
            output,
            output_capacity,
            count_out,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_engine_process_track_fanout(
    state: *const c_void,
    source_id: u32,
    source_channels: *const *const f32,
    source_channel_count: u32,
    source_send_pre_left: *const f32,
    source_send_pre_right: *const f32,
    source_send_post_left: *const f32,
    source_send_post_right: *const f32,
    source_track_delay_samples: u32,
    destinations: *const crate::routing_fanout::RoutingTrackDestination,
    destination_count: usize,
    master_left: *mut f32,
    master_right: *mut f32,
    send_scratch_left: *mut f32,
    send_scratch_right: *mut f32,
    frames: u32,
    max_send_frames: u32,
    add_bus_pre_fx: Option<crate::routing_fanout::BusPreFxAdd>,
) {
    let Some(core) = (unsafe { engine(state) }) else { return };
    unsafe {
        crate::routing_fanout::hirari_routing_process_track_fanout(
            core.gains,
            core.send_pdc,
            source_id,
            source_channels,
            source_channel_count,
            source_send_pre_left,
            source_send_pre_right,
            source_send_post_left,
            source_send_post_right,
            source_track_delay_samples,
            destinations,
            destination_count,
            master_left,
            master_right,
            send_scratch_left,
            send_scratch_right,
            frames,
            max_send_frames,
            add_bus_pre_fx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connections(state: *const c_void) -> Vec<RoutingConnectionRecord> {
        let mut count = 0usize;
        let records = unsafe { hirari_routing_engine_copy_connections(state, &mut count) };
        if records.is_null() {
            return Vec::new();
        }
        let copied = unsafe { std::slice::from_raw_parts(records, count) }
            .iter()
            .map(|record| RoutingConnectionRecord {
                source_id: record.source_id,
                destination_id: record.destination_id,
                gain: record.gain,
                send: record.send,
                pre_fader: record.pre_fader,
            })
            .collect();
        unsafe { hirari_routing_connection_array_free(records, count) };
        copied
    }

    #[test]
    fn engine_owns_the_graph_and_returns_removed_edges() {
        let state = hirari_routing_engine_create();
        assert!(!state.is_null());
        assert!(unsafe { hirari_routing_engine_add_connection(state, 1, 2, 0.5, false, false) });
        assert!(unsafe { hirari_routing_engine_add_connection(state, 1, 3, 0.25, true, true) });
        let taken = connections(state);
        assert_eq!(taken.len(), 2);
        assert!(unsafe { hirari_routing_engine_has_connection(state, 1, 2) });
        assert!((unsafe { hirari_routing_engine_send_gain(state, 1, 3) } - 0.25).abs() < 1.0e-6);

        let mut count = 0usize;
        let removed = unsafe { hirari_routing_engine_remove_connections_for_node(state, 1, &mut count) };
        assert!(!removed.is_null());
        unsafe { hirari_routing_connection_array_free(removed, count) };
        assert!(connections(state).is_empty());
        unsafe { hirari_routing_engine_destroy(state) };
    }

    #[test]
    fn feedback_edges_round_trip_and_reset_clears_every_store() {
        let state = hirari_routing_engine_create();
        assert!(unsafe { hirari_routing_engine_add_feedback(state, 4, 5, 0.5) });
        assert!((unsafe { hirari_routing_engine_feedback_gain(state, 4, 5) } - 0.5).abs() < 1.0e-6);
        let mut count = 0usize;
        let copied = unsafe { hirari_routing_engine_copy_feedback(state, &mut count) };
        assert_eq!(count, 1);
        assert!(!copied.is_null());
        unsafe { hirari_routing_feedback_array_free(copied, count) };

        unsafe { hirari_routing_engine_reset(state) };
        assert!((unsafe { hirari_routing_engine_feedback_gain(state, 4, 5) }) < 0.0);
        unsafe { hirari_routing_engine_destroy(state) };
    }

    #[test]
    fn degenerate_nodes_and_handles_do_nothing() {
        assert!(!unsafe { hirari_routing_engine_add_connection(std::ptr::null_mut(), 1, 2, 1.0, false, false) });
        assert!(!unsafe { hirari_routing_engine_has_connection(std::ptr::null(), 1, 2) });
        let state = hirari_routing_engine_create();
        let mut count = 0usize;
        let out = unsafe { hirari_routing_engine_remove_connections_for_node(state, MAX_NODES, &mut count) };
        assert!(out.is_null());
        assert_eq!(count, 0);
        unsafe { hirari_routing_engine_destroy(state) };
    }
}
