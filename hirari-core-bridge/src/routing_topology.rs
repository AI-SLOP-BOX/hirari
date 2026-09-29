use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const MAX_ROUTING_NODES: usize = 128;
const MAX_ACTIVE_TRACKS: usize = 256;

/// Rust-owned publication cache read by the realtime track-routing path.
/// Graph compilation and publication happen on the control side; readers only
/// observe a bounded atomic snapshot and never wait for that writer.
pub struct RoutingTopologyState {
    nodes: [AtomicU32; MAX_ROUTING_NODES],
    count: AtomicU32,
    generation: AtomicU64,
    dirty: AtomicBool,
}

impl Default for RoutingTopologyState {
    fn default() -> Self {
        Self {
            nodes: std::array::from_fn(|_| AtomicU32::new(0)),
            count: AtomicU32::new(0),
            generation: AtomicU64::new(0),
            dirty: AtomicBool::new(true),
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_routing_topology_create() -> *mut c_void {
    Box::into_raw(Box::new(RoutingTopologyState::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<RoutingTopologyState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_mark_dirty(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<RoutingTopologyState>().as_ref() } {
        state.dirty.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_reset(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<RoutingTopologyState>().as_ref() }) else {
        return;
    };
    state.count.store(0, Ordering::Release);
    state.dirty.store(true, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_build(
    topology: *mut c_void,
    graph: *const c_void,
) -> bool {
    let Some(topology) = (unsafe { topology.cast::<RoutingTopologyState>().as_ref() }) else {
        return false;
    };
    if graph.is_null() {
        return false;
    }

    // Serialize topology writers without making the audio reader wait. An
    // odd generation marks an in-progress publication.
    let generation = topology.generation.load(Ordering::Acquire);
    if generation & 1 != 0
        || topology
            .generation
            .compare_exchange(
                generation,
                generation.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
    {
        return false;
    }

    let mut compiled = [0u32; MAX_ROUTING_NODES];
    let compiled_count = unsafe {
        crate::routing_graph_pdc::hirari_routing_graph_build_order(
            graph,
            compiled.as_mut_ptr(),
            compiled.len(),
        )
    };
    if compiled_count > MAX_ROUTING_NODES {
        topology.generation.fetch_add(1, Ordering::Release);
        return false;
    }

    for (target, source) in topology
        .nodes
        .iter()
        .zip(compiled.iter())
        .take(compiled_count)
    {
        target.store(*source, Ordering::Relaxed);
    }
    topology
        .count
        .store(compiled_count as u32, Ordering::Relaxed);
    topology.generation.fetch_add(1, Ordering::Release);
    topology.dirty.store(false, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_copy(
    state: *const c_void,
    output: *mut u32,
    output_capacity: usize,
    count_out: *mut u32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<RoutingTopologyState>().as_ref() }) else {
        return false;
    };
    if output.is_null() || count_out.is_null() || output_capacity < MAX_ROUTING_NODES {
        return false;
    }
    if state.dirty.load(Ordering::Acquire) {
        unsafe { count_out.write(0) };
        return false;
    }
    let generation = state.generation.load(Ordering::Acquire);
    if generation & 1 != 0 {
        unsafe { count_out.write(0) };
        return false;
    }
    let count = state.count.load(Ordering::Acquire) as usize;
    if count > MAX_ROUTING_NODES {
        unsafe { count_out.write(0) };
        return false;
    }
    for index in 0..count {
        unsafe {
            output
                .add(index)
                .write(state.nodes[index].load(Ordering::Acquire))
        };
    }
    if state.generation.load(Ordering::Acquire) != generation {
        unsafe { count_out.write(0) };
        return false;
    }
    unsafe { count_out.write(count as u32) };
    true
}

/// Resolves the complete block processing order from the compiled routing
/// order plus the current active-track snapshot. Connected nodes keep their
/// topological order; active tracks omitted from the graph are appended in
/// engine order. While a graph edit is being published, it safely falls back
/// to the active-track order, matching the host's previous second pass.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_resolve_process_order(
    state: *const c_void,
    active_ids: *const u32,
    active_count: usize,
    output: *mut u32,
    output_capacity: usize,
    count_out: *mut u32,
) -> bool {
    if state.is_null()
        || count_out.is_null()
        || (active_count != 0 && active_ids.is_null())
        || active_count > MAX_ACTIVE_TRACKS
        || output.is_null()
        || output_capacity < MAX_ACTIVE_TRACKS + MAX_ROUTING_NODES
    {
        if !count_out.is_null() {
            unsafe { count_out.write(0) };
        }
        return false;
    }
    let mut compiled = [0_u32; MAX_ROUTING_NODES];
    let mut compiled_count = 0_u32;
    let has_stable_graph = unsafe {
        hirari_routing_topology_copy(
            state,
            compiled.as_mut_ptr(),
            compiled.len(),
            &mut compiled_count,
        )
    };
    let active = if active_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(active_ids, active_count) }
    };
    let output = unsafe { std::slice::from_raw_parts_mut(output, output_capacity) };
    let mut count = 0usize;

    if has_stable_graph {
        for &node in compiled.iter().take(compiled_count as usize) {
            output[count] = node;
            count += 1;
        }
    }
    for &track_id in active {
        if has_stable_graph && compiled[..compiled_count as usize].contains(&track_id) {
            continue;
        }
        output[count] = track_id;
        count += 1;
    }
    unsafe { count_out.write(count as u32) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_topology_node_count(state: *const c_void) -> u32 {
    let Some(state) = (unsafe { state.cast::<RoutingTopologyState>().as_ref() }) else {
        return 0;
    };
    if state.dirty.load(Ordering::Acquire) {
        return 0;
    }
    state
        .count
        .load(Ordering::Acquire)
        .min(MAX_ROUTING_NODES as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_publishes_graph_order_and_hides_it_after_mutation() {
        let graph = crate::routing_graph_pdc::hirari_routing_graph_create();
        let topology = hirari_routing_topology_create();
        assert!(unsafe {
            crate::routing_graph_pdc::hirari_routing_graph_add_connection(
                graph, 3, 5, 1.0, false, false,
            )
        });
        assert!(unsafe { hirari_routing_topology_build(topology, graph) });

        let mut output = [u32::MAX; MAX_ROUTING_NODES];
        let mut count = 0;
        assert!(unsafe {
            hirari_routing_topology_copy(topology, output.as_mut_ptr(), output.len(), &mut count)
        });
        assert_eq!(&output[..count as usize], &[3, 5]);

        unsafe { hirari_routing_topology_mark_dirty(topology) };
        assert!(!unsafe {
            hirari_routing_topology_copy(topology, output.as_mut_ptr(), output.len(), &mut count)
        });
        assert_eq!(count, 0);
        unsafe {
            hirari_routing_topology_destroy(topology);
            crate::routing_graph_pdc::hirari_routing_graph_destroy(graph);
        }
    }

    #[test]
    fn process_order_appends_isolated_tracks_and_uses_fallback_during_edits() {
        unsafe {
            let graph = crate::routing_graph_pdc::hirari_routing_graph_create();
            let topology = hirari_routing_topology_create();
            assert!(
                crate::routing_graph_pdc::hirari_routing_graph_add_connection(
                    graph, 3, 5, 1.0, false, false,
                )
            );
            assert!(hirari_routing_topology_build(topology, graph));

            let active = [5_u32, 9, 3, 12];
            let mut output = [u32::MAX; MAX_ACTIVE_TRACKS + MAX_ROUTING_NODES];
            let mut count = 0;
            assert!(hirari_routing_topology_resolve_process_order(
                topology,
                active.as_ptr(),
                active.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut count,
            ));
            assert_eq!(&output[..count as usize], &[3, 5, 9, 12]);

            hirari_routing_topology_mark_dirty(topology);
            assert!(hirari_routing_topology_resolve_process_order(
                topology,
                active.as_ptr(),
                active.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut count,
            ));
            assert_eq!(&output[..count as usize], &active);

            hirari_routing_topology_destroy(topology);
            crate::routing_graph_pdc::hirari_routing_graph_destroy(graph);
        }
    }
}
