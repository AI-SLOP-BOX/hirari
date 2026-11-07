use std::ffi::c_void;

const MAX_ROUTING_NODES: u32 = 128;

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_add_connection(
    graph: *const c_void,
    gains: *mut c_void,
    topology: *mut c_void,
    source: u32,
    destination: u32,
    gain: f32,
    send: bool,
    pre_fader: bool,
) -> bool {
    if graph.is_null()
        || gains.is_null()
        || topology.is_null()
        || source == destination
        || source >= MAX_ROUTING_NODES
        || destination >= MAX_ROUTING_NODES
        || !gain.is_finite()
    {
        return false;
    }
    let gain = gain.clamp(0.0, 2.0);
    if !unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_add_connection(
            graph,
            source,
            destination,
            gain,
            send,
            send && pre_fader,
        )
    } {
        return false;
    }
    if send {
        unsafe {
            crate::routing_gains::hirari_routing_gains_set_send(
                gains,
                source,
                destination,
                gain,
                pre_fader,
            )
        };
    } else {
        unsafe {
            crate::routing_gains::hirari_routing_gains_set_route(gains, source, destination, gain)
        };
    }
    unsafe { crate::routing_topology::hirari_routing_topology_mark_dirty(topology) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_remove_connection(
    graph: *const c_void,
    gains: *mut c_void,
    topology: *mut c_void,
    source: u32,
    destination: u32,
) {
    if graph.is_null()
        || gains.is_null()
        || topology.is_null()
        || source >= MAX_ROUTING_NODES
        || destination >= MAX_ROUTING_NODES
    {
        return;
    }
    unsafe {
        crate::routing_gains::hirari_routing_gains_clear_route(gains, source, destination);
        crate::routing_graph_pdc::hirari_routing_graph_remove_connection(
            graph,
            source,
            destination,
            false,
        );
        crate::routing_topology::hirari_routing_topology_mark_dirty(topology);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_remove_send(
    graph: *const c_void,
    gains: *mut c_void,
    send_pdc: *mut c_void,
    topology: *mut c_void,
    source: u32,
    destination: u32,
) {
    if graph.is_null()
        || gains.is_null()
        || send_pdc.is_null()
        || topology.is_null()
        || source >= MAX_ROUTING_NODES
        || destination >= MAX_ROUTING_NODES
    {
        return;
    }
    unsafe {
        crate::routing_gains::hirari_routing_gains_clear_send(gains, source, destination);
        crate::send_pdc_manager::hirari_send_pdc_manager_retire(send_pdc, source, destination);
        crate::send_pdc_manager::hirari_send_pdc_manager_set_delay(
            send_pdc,
            source,
            destination,
            0,
        );
        crate::routing_graph_pdc::hirari_routing_graph_remove_connection(
            graph,
            source,
            destination,
            true,
        );
        crate::routing_topology::hirari_routing_topology_mark_dirty(topology);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_add_dependency(
    graph: *const c_void,
    topology: *mut c_void,
    source: u32,
    destination: u32,
) -> bool {
    if graph.is_null()
        || topology.is_null()
        || source == destination
        || source >= MAX_ROUTING_NODES
        || destination >= MAX_ROUTING_NODES
    {
        return false;
    }
    if !unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_add_dependency(graph, source, destination)
    } {
        return false;
    }
    unsafe { crate::routing_topology::hirari_routing_topology_mark_dirty(topology) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_remove_dependency(
    graph: *const c_void,
    topology: *mut c_void,
    source: u32,
    destination: u32,
) {
    if graph.is_null() || topology.is_null() {
        return;
    }
    if unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_remove_dependency(graph, source, destination)
    } {
        unsafe { crate::routing_topology::hirari_routing_topology_mark_dirty(topology) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_remove_dependencies_for_node(
    graph: *const c_void,
    topology: *mut c_void,
    node: u32,
) {
    if graph.is_null() || topology.is_null() {
        return;
    }
    if unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_remove_dependencies_for_node(graph, node)
    } {
        unsafe { crate::routing_topology::hirari_routing_topology_mark_dirty(topology) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_reset(
    graph: *const c_void,
    gains: *const c_void,
    feedback: *const c_void,
    send_pdc: *mut c_void,
    topology: *mut c_void,
) {
    if graph.is_null()
        || gains.is_null()
        || feedback.is_null()
        || send_pdc.is_null()
        || topology.is_null()
    {
        return;
    }
    unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_reset(graph);
        crate::routing_gains::hirari_routing_gains_reset(gains);
        crate::routing_feedback::hirari_routing_feedback_reset(feedback);
        crate::send_pdc_manager::hirari_send_pdc_manager_retire_all(send_pdc);
        crate::routing_topology::hirari_routing_topology_reset(topology);
    }
}

/// Remove a node's graph edges, synchronize their dependent routing state, and
/// return the removed records as the undo snapshot.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_runtime_remove_connections_for_node(
    graph: *const c_void,
    gains: *mut c_void,
    send_pdc: *mut c_void,
    topology: *mut c_void,
    node: u32,
) -> *mut c_void {
    if graph.is_null() || gains.is_null() || send_pdc.is_null() || topology.is_null() {
        return std::ptr::null_mut();
    }

    let snapshot = unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_remove_connections_for_node(graph, node)
    };
    if snapshot.is_null() {
        return std::ptr::null_mut();
    }
    let removed =
        unsafe { &*snapshot.cast::<Vec<crate::routing_graph_pdc::RoutingConnectionRecord>>() };
    for connection in removed {
        if connection.send {
            unsafe {
                crate::routing_gains::hirari_routing_gains_clear_send(
                    gains,
                    connection.source_id,
                    connection.destination_id,
                );
                crate::send_pdc_manager::hirari_send_pdc_manager_retire(
                    send_pdc,
                    connection.source_id,
                    connection.destination_id,
                );
                crate::send_pdc_manager::hirari_send_pdc_manager_set_delay(
                    send_pdc,
                    connection.source_id,
                    connection.destination_id,
                    0,
                );
            }
        } else {
            unsafe {
                crate::routing_gains::hirari_routing_gains_clear_route(
                    gains,
                    connection.source_id,
                    connection.destination_id,
                );
            }
        }
    }
    if !removed.is_empty() {
        unsafe { crate::routing_topology::hirari_routing_topology_mark_dirty(topology) };
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_mutations_update_graph_gain_and_topology_as_one_operation() {
        unsafe {
            let graph = crate::routing_graph_pdc::hirari_routing_graph_create();
            let gains = crate::routing_gains::hirari_routing_gains_create();
            let topology = crate::routing_topology::hirari_routing_topology_create();
            let send_pdc = crate::send_pdc_manager::hirari_send_pdc_manager_create();
            let feedback = crate::routing_feedback::hirari_routing_feedback_create();

            assert!(hirari_routing_runtime_add_connection(
                graph, gains, topology, 2, 7, 4.0, false, true,
            ));
            assert_eq!(
                crate::routing_gains::hirari_routing_gains_route(gains, 2, 7),
                2.0
            );
            assert_eq!(
                crate::routing_topology::hirari_routing_topology_node_count(topology),
                0
            );
            assert!(crate::routing_topology::hirari_routing_topology_build(
                topology, graph
            ));
            assert_eq!(
                crate::routing_topology::hirari_routing_topology_node_count(topology),
                2
            );

            hirari_routing_runtime_remove_connection(graph, gains, topology, 2, 7);
            assert_eq!(
                crate::routing_gains::hirari_routing_gains_route(gains, 2, 7),
                0.0
            );
            assert_eq!(
                crate::routing_topology::hirari_routing_topology_node_count(topology),
                0
            );

            assert!(hirari_routing_runtime_add_connection(
                graph, gains, topology, 2, 7, 0.5, true, true,
            ));
            assert!(crate::routing_gains::hirari_routing_gains_send_pre_fader(
                gains, 2, 7
            ));
            assert!(crate::send_pdc_manager::hirari_send_pdc_manager_set_delay(
                send_pdc, 2, 7, 16,
            ));
            hirari_routing_runtime_remove_send(graph, gains, send_pdc, topology, 2, 7);
            assert!(!crate::routing_gains::hirari_routing_gains_has_send(
                gains, 2, 7
            ));

            hirari_routing_runtime_reset(graph, gains, feedback, send_pdc, topology);
            crate::routing_graph_pdc::hirari_routing_graph_destroy(graph);
            crate::routing_gains::hirari_routing_gains_destroy(gains);
            crate::routing_topology::hirari_routing_topology_destroy(topology);
            crate::send_pdc_manager::hirari_send_pdc_manager_destroy(send_pdc);
            crate::routing_feedback::hirari_routing_feedback_destroy(feedback);
        }
    }

    #[test]
    fn node_removal_clears_route_and_send_state_and_invalidates_topology() {
        unsafe {
            let graph = crate::routing_graph_pdc::hirari_routing_graph_create();
            let gains = crate::routing_gains::hirari_routing_gains_create();
            let topology = crate::routing_topology::hirari_routing_topology_create();
            let send_pdc = crate::send_pdc_manager::hirari_send_pdc_manager_create();
            crate::routing_graph_pdc::hirari_routing_graph_add_connection(
                graph, 2, 9, 0.75, false, false,
            );
            crate::routing_graph_pdc::hirari_routing_graph_add_connection(
                graph, 4, 9, 0.5, true, true,
            );
            crate::routing_gains::hirari_routing_gains_set_route(gains, 2, 9, 0.75);
            crate::routing_gains::hirari_routing_gains_set_send(gains, 4, 9, 0.5, true);
            crate::send_pdc_manager::hirari_send_pdc_manager_set_delay(send_pdc, 4, 9, 24);
            assert!(crate::routing_topology::hirari_routing_topology_build(
                topology, graph
            ));

            let removed = hirari_routing_runtime_remove_connections_for_node(
                graph, gains, send_pdc, topology, 9,
            );
            let count = crate::routing_graph_pdc::hirari_routing_graph_snapshot_count(removed);
            let mut records =
                vec![crate::routing_graph_pdc::RoutingConnectionRecord::default(); count];
            let copied = crate::routing_graph_pdc::hirari_routing_graph_snapshot_copy(
                removed,
                records.as_mut_ptr().cast(),
                records.len(),
            );
            crate::routing_graph_pdc::hirari_routing_graph_snapshot_destroy(removed);
            assert_eq!(copied, 2);

            assert_eq!(
                records
                    .iter()
                    .map(|record| record.source_id)
                    .collect::<Vec<_>>(),
                [2, 4]
            );
            assert_eq!(
                crate::routing_gains::hirari_routing_gains_route(gains, 2, 9),
                0.0
            );
            assert!(!crate::routing_gains::hirari_routing_gains_has_send(
                gains, 4, 9
            ));
            assert_eq!(
                crate::routing_topology::hirari_routing_topology_node_count(topology),
                0
            );

            crate::routing_graph_pdc::hirari_routing_graph_destroy(graph);
            crate::routing_gains::hirari_routing_gains_destroy(gains);
            crate::routing_topology::hirari_routing_topology_destroy(topology);
            crate::send_pdc_manager::hirari_send_pdc_manager_destroy(send_pdc);
        }
    }
}
