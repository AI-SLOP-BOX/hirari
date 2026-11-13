use std::collections::{BTreeMap, BTreeSet};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

const SEND_PDC_MAX_DELAY: u32 = 8192;
const SEND_PDC_CAPACITY: usize = SEND_PDC_MAX_DELAY as usize + 1;

/// Compiles node execution order and longest-path stage depths for the legacy
/// graph adapter. Output depths align with `node_ids`; execution order is
/// deterministic, choosing the lowest ready node ID at each step.
#[no_mangle]
pub unsafe extern "C" fn hirari_compile_staged_routing_graph(
    node_ids: *const u32,
    node_count: usize,
    edge_sources: *const u32,
    edge_destinations: *const u32,
    edge_count: usize,
    execution_order_out: *mut u32,
    order_capacity: usize,
    stage_depths_out: *mut u32,
    depth_capacity: usize,
) -> bool {
    if (node_count > 0
        && (node_ids.is_null()
            || execution_order_out.is_null()
            || stage_depths_out.is_null()
            || order_capacity < node_count
            || depth_capacity < node_count))
        || (edge_count > 0 && (edge_sources.is_null() || edge_destinations.is_null()))
    {
        return false;
    }
    let ids = if node_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(node_ids, node_count)
    };
    let sources = if edge_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(edge_sources, edge_count)
    };
    let destinations = if edge_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(edge_destinations, edge_count)
    };
    let mut outgoing = BTreeMap::<u32, Vec<u32>>::new();
    let mut indegree = BTreeMap::<u32, u32>::new();
    let mut depths = BTreeMap::<u32, u32>::new();
    for id in ids {
        if outgoing.insert(*id, Vec::new()).is_some() {
            return false;
        }
        indegree.insert(*id, 0);
        depths.insert(*id, 0);
    }
    for (source, destination) in sources.iter().zip(destinations) {
        let Some(children) = outgoing.get_mut(source) else {
            return false;
        };
        let Some(degree) = indegree.get_mut(destination) else {
            return false;
        };
        if children.contains(destination) {
            continue;
        }
        let Some(next) = degree.checked_add(1) else {
            return false;
        };
        *degree = next;
        children.push(*destination);
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(node_count);
    while let Some(id) = ready.pop_first() {
        order.push(id);
        let parent_depth = depths[&id];
        for destination in &outgoing[&id] {
            let Some(depth) = depths.get_mut(destination) else {
                return false;
            };
            *depth = (*depth).max(parent_depth.saturating_add(1));
            let Some(degree) = indegree.get_mut(destination) else {
                return false;
            };
            let Some(next) = degree.checked_sub(1) else {
                return false;
            };
            *degree = next;
            if next == 0 {
                ready.insert(*destination);
            }
        }
    }
    if order.len() != node_count {
        return false;
    }
    if node_count > 0 {
        let stage_depths = std::slice::from_raw_parts_mut(stage_depths_out, node_count);
        for (index, id) in ids.iter().enumerate() {
            stage_depths[index] = depths[id];
        }
        std::slice::from_raw_parts_mut(execution_order_out, node_count).copy_from_slice(&order);
    }
    true
}

#[path = "routing_send_pdc.rs"]
mod send_pdc;
pub(crate) use send_pdc::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RoutingConnectionRecord {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
    pub send: bool,
    pub pre_fader: bool,
}
const _: [(); 16] = [(); std::mem::size_of::<RoutingConnectionRecord>()];

#[derive(Default)]
struct RoutingGraphData {
    connections: Vec<RoutingConnectionRecord>,
    processing_dependencies: Vec<(u32, u32)>,
}

struct RoutingGraphState {
    data: Mutex<RoutingGraphData>,
}

unsafe fn routing_graph_state(handle: *const c_void) -> Option<&'static RoutingGraphState> {
    if handle.is_null() {
        None
    } else {
        // SAFETY: this handle is allocated and freed by the graph FFI below.
        Some(unsafe { &*handle.cast::<RoutingGraphState>() })
    }
}

fn graph_would_create_cycle(data: &RoutingGraphData, source: u32, destination: u32) -> bool {
    let mut edges =
        Vec::with_capacity((data.connections.len() + data.processing_dependencies.len()) * 2);
    for connection in &data.connections {
        edges.push(connection.source_id);
        edges.push(connection.destination_id);
    }
    for (dependency_source, dependency_destination) in &data.processing_dependencies {
        edges.push(*dependency_source);
        edges.push(*dependency_destination);
    }
    // SAFETY: `edges` is a valid packed source/destination array for this call.
    unsafe {
        hirari_routing_would_create_cycle(
            if edges.is_empty() {
                std::ptr::null()
            } else {
                edges.as_ptr()
            },
            edges.len() / 2,
            source,
            destination,
        )
    }
}

#[no_mangle]
pub extern "C" fn hirari_routing_graph_create() -> *mut c_void {
    Box::into_raw(Box::new(RoutingGraphState {
        data: Mutex::new(RoutingGraphData::default()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: the handle was returned by `hirari_routing_graph_create` and
        // the owner destroys it exactly once after graph operations stop.
        drop(unsafe { Box::from_raw(handle.cast::<RoutingGraphState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_add_connection(
    handle: *const c_void,
    source: u32,
    destination: u32,
    gain: f32,
    send: bool,
    pre_fader: bool,
) -> bool {
    const MAX_NODES: u32 = 128;
    if source >= MAX_NODES || destination >= MAX_NODES || source == destination || !gain.is_finite()
    {
        return false;
    }
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return false;
    };
    let Ok(mut data) = state.data.lock() else {
        return false;
    };
    let existing = data.connections.iter().position(|connection| {
        connection.source_id == source
            && connection.destination_id == destination
            && connection.send == send
    });
    if existing.is_none() && graph_would_create_cycle(&data, source, destination) {
        return false;
    }
    let connection = RoutingConnectionRecord {
        source_id: source,
        destination_id: destination,
        gain: gain.clamp(0.0, 2.0),
        send,
        pre_fader: send && pre_fader,
    };
    if let Some(index) = existing {
        data.connections[index] = connection;
    } else {
        data.connections.push(connection);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_remove_connection(
    handle: *const c_void,
    source: u32,
    destination: u32,
    send: bool,
) {
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return;
    };
    if let Ok(mut data) = state.data.lock() {
        data.connections.retain(|connection| {
            connection.source_id != source
                || connection.destination_id != destination
                || connection.send != send
        });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_add_dependency(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    const MAX_NODES: u32 = 128;
    if source >= MAX_NODES || destination >= MAX_NODES || source == destination {
        return false;
    }
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return false;
    };
    let Ok(mut data) = state.data.lock() else {
        return false;
    };
    let edge = (source, destination);
    if data.processing_dependencies.contains(&edge) {
        return true;
    }
    if graph_would_create_cycle(&data, source, destination) {
        return false;
    }
    data.processing_dependencies.push(edge);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_remove_dependency(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return false;
    };
    let Ok(mut data) = state.data.lock() else {
        return false;
    };
    let previous_len = data.processing_dependencies.len();
    data.processing_dependencies
        .retain(|edge| *edge != (source, destination));
    data.processing_dependencies.len() != previous_len
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_has_dependency(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return false;
    };
    state
        .data
        .lock()
        .is_ok_and(|data| data.processing_dependencies.contains(&(source, destination)))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_remove_dependencies_for_node(
    handle: *const c_void,
    node: u32,
) -> bool {
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return false;
    };
    let Ok(mut data) = state.data.lock() else {
        return false;
    };
    let previous_len = data.processing_dependencies.len();
    data.processing_dependencies
        .retain(|edge| edge.0 != node && edge.1 != node);
    data.processing_dependencies.len() != previous_len
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_reset(handle: *const c_void) {
    if let Some(state) = unsafe { routing_graph_state(handle) } {
        if let Ok(mut data) = state.data.lock() {
            data.connections.clear();
            data.processing_dependencies.clear();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_remove_connections_for_node(
    handle: *const c_void,
    node: u32,
) -> *mut c_void {
    let removed = if let Some(state) = unsafe { routing_graph_state(handle) } {
        if let Ok(mut data) = state.data.lock() {
            let mut removed = Vec::new();
            let mut retained = Vec::with_capacity(data.connections.len());
            for connection in data.connections.drain(..) {
                if connection.source_id == node || connection.destination_id == node {
                    removed.push(connection);
                } else {
                    retained.push(connection);
                }
            }
            data.connections = retained;
            removed
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    Box::into_raw(Box::new(removed)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_snapshot_create(
    handle: *const c_void,
) -> *mut c_void {
    let snapshot = unsafe { routing_graph_state(handle) }
        .and_then(|state| state.data.lock().ok().map(|data| data.connections.clone()))
        .unwrap_or_default();
    Box::into_raw(Box::new(snapshot)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_snapshot_count(snapshot: *const c_void) -> usize {
    if snapshot.is_null() {
        return 0;
    }
    unsafe { (&*snapshot.cast::<Vec<RoutingConnectionRecord>>()).len() }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_snapshot_copy(
    snapshot: *const c_void,
    output: *mut c_void,
    capacity: usize,
) -> usize {
    if snapshot.is_null() {
        return 0;
    }
    let connections = unsafe { &*snapshot.cast::<Vec<RoutingConnectionRecord>>() };
    let copied = connections.len().min(capacity);
    if copied > 0 && output.is_null() {
        return 0;
    }
    if copied > 0 {
        // SAFETY: caller provides `capacity` writable records, and the Rust
        // and C++ record layouts are asserted at the adapter boundary.
        unsafe {
            std::ptr::copy_nonoverlapping(
                connections.as_ptr(),
                output.cast::<RoutingConnectionRecord>(),
                copied,
            );
        }
    }
    copied
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_snapshot_destroy(snapshot: *mut c_void) {
    if !snapshot.is_null() {
        // SAFETY: this boxed snapshot was created by one of the snapshot APIs.
        drop(unsafe { Box::from_raw(snapshot.cast::<Vec<RoutingConnectionRecord>>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_snapshot_json(handle: *const c_void) -> *mut std::ffi::c_char {
    let connections = unsafe { routing_graph_state(handle) }
        .and_then(|state| state.data.lock().ok().map(|data| data.connections.clone()))
        .unwrap_or_default();
    let mut out = String::from("[");
    for (i, conn) in connections.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        use std::fmt::Write;
        let _ = write!(
            out,
            r#"{{"source_id":{},"destination_id":{},"gain":{},"send":{},"pre_fader":{}}}"#,
            conn.source_id,
            conn.destination_id,
            if conn.gain.is_finite() { conn.gain } else { 0.0 },
            conn.send,
            conn.pre_fader
        );
    }
    out.push(']');
    std::ffi::CString::new(out).map_or(std::ptr::null_mut(), std::ffi::CString::into_raw)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_json_free(ptr: *mut std::ffi::c_char) {
    if !ptr.is_null() {
        drop(unsafe { std::ffi::CString::from_raw(ptr) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_graph_build_order(
    handle: *const c_void,
    output: *mut u32,
    output_capacity: usize,
) -> usize {
    const MAX_NODES: usize = 128;
    if output.is_null() || output_capacity < MAX_NODES {
        return usize::MAX;
    }
    let Some(state) = (unsafe { routing_graph_state(handle) }) else {
        return usize::MAX;
    };
    let Ok(data) = state.data.lock() else {
        return usize::MAX;
    };
    let mut edges =
        Vec::with_capacity((data.connections.len() + data.processing_dependencies.len()) * 2);
    for connection in &data.connections {
        edges.push(connection.source_id);
        edges.push(connection.destination_id);
    }
    for (source, destination) in &data.processing_dependencies {
        edges.push(*source);
        edges.push(*destination);
    }
    // SAFETY: the packed edges are valid and output has capacity for all nodes.
    unsafe {
        hirari_compile_routing_order(
            if edges.is_empty() {
                std::ptr::null()
            } else {
                edges.as_ptr()
            },
            edges.len() / 2,
            output,
            output_capacity,
        )
    }
}

pub struct AudioNodeRust {
    pub id: u32,
    pub processing_latency: u32,
    pub cumulative_delay: u32,
    pub outgoing_edges: Vec<u32>,
    pub in_degree: u32,
}

pub struct RoutingOrchestrator {
    pub nodes: HashMap<u32, AudioNodeRust>,
    pub execution_order: Vec<u32>,
}

impl Default for RoutingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingOrchestrator {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            execution_order: Vec::new(),
        }
    }

    /// INDUSTRIAL: Compiles the routing graph with absolute topological precision and Kahn's Algorithm.
    pub fn compile_graph(&mut self) -> bool {
        // INDUSTRIAL: Implementation of high-performance topological sorting.
        // Rust's DependencySortEngine ensures bit-accurate routing distribution.
        self.execution_order.clear();
        let mut in_degrees = HashMap::new();
        let mut no_incoming = VecDeque::new();

        // Reject dangling edges before the topological sort.  Apart from being
        // an invalid graph, such an edge has no entry in `in_degrees` and must
        // not be allowed to turn the error path into a panic.
        for node in self.nodes.values() {
            if node
                .outgoing_edges
                .iter()
                .any(|neighbor| !self.nodes.contains_key(neighbor))
            {
                return false;
            }
        }

        // Derive indegrees from the edge list rather than trusting a stale
        // serialized field. This keeps graph compilation deterministic after
        // edits and also repairs the cached node metadata.
        for &id in self.nodes.keys() {
            in_degrees.insert(id, 0_u32);
        }
        for node in self.nodes.values() {
            for &neighbor in &node.outgoing_edges {
                let Some(degree) = in_degrees.get_mut(&neighbor) else {
                    return false;
                };
                let Some(next) = degree.checked_add(1) else {
                    return false;
                };
                *degree = next;
            }
        }
        for (&id, degree) in &in_degrees {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.in_degree = *degree;
            }
            if *degree == 0 {
                no_incoming.push_back(id);
            }
        }

        while let Some(curr) = no_incoming.pop_front() {
            self.execution_order.push(curr);
            if let Some(node) = self.nodes.get(&curr) {
                for &neighbor in &node.outgoing_edges {
                    let Some(degree) = in_degrees.get_mut(&neighbor) else {
                        return false;
                    };
                    let Some(next_degree) = degree.checked_sub(1) else {
                        return false;
                    };
                    *degree = next_degree;
                    if *degree == 0 {
                        no_incoming.push_back(neighbor);
                    }
                }
            }
        }

        if self.execution_order.len() != self.nodes.len() {
            return false;
        }

        // --- PDC: DELAY COMPENSATION CALCULATION ---
        let mut path_delays = HashMap::new();
        for &curr in self.execution_order.iter().rev() {
            let mut max_child_path = 0;
            if let Some(node) = self.nodes.get(&curr) {
                for &neighbor in &node.outgoing_edges {
                    max_child_path = max_child_path.max(*path_delays.get(&neighbor).unwrap_or(&0));
                }
                let Some(path_delay) = node.processing_latency.checked_add(max_child_path) else {
                    return false;
                };
                path_delays.insert(curr, path_delay);
            }
        }

        let mut branch_delays = HashMap::<u32, u32>::new();
        for (&_curr, node) in &self.nodes {
            let mut max_child_path = 0;
            for &neighbor in &node.outgoing_edges {
                max_child_path = max_child_path.max(*path_delays.get(&neighbor).unwrap_or(&0));
            }

            for &neighbor in &node.outgoing_edges {
                let neighbor_path = *path_delays.get(&neighbor).unwrap_or(&0);
                let _diff = max_child_path.saturating_sub(neighbor_path);
                // Store the delay needed to align this branch with the
                // longest downstream path. This is the value consumed by the
                // realtime PDC scheduler.
                let entry = branch_delays.entry(neighbor).or_insert(0);
                *entry = (*entry).max(_diff);
            }
        }
        for (id, delay) in branch_delays {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.cumulative_delay = delay;
            }
        }

        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_routing_graph_pdc(&self) -> bool {
        let mut incoming = HashMap::<u32, u32>::new();
        for (&id, node) in &self.nodes {
            if node.id != id || node.processing_latency > 1_000_000 {
                return false;
            }
            if node.cumulative_delay > 1_000_000
                || node
                    .outgoing_edges
                    .iter()
                    .any(|edge| !self.nodes.contains_key(edge))
            {
                return false;
            }
            for &edge in &node.outgoing_edges {
                if edge == id
                    || node
                        .outgoing_edges
                        .iter()
                        .filter(|candidate| **candidate == edge)
                        .count()
                        > 1
                {
                    return false;
                }
                let count = incoming.entry(edge).or_insert(0);
                *count = count.saturating_add(1);
            }
        }
        if self
            .nodes
            .iter()
            .any(|(id, node)| node.in_degree != incoming.get(id).copied().unwrap_or(0))
        {
            return false;
        }
        let mut seen = std::collections::HashSet::new();
        self.execution_order.len() == self.nodes.len()
            && self
                .execution_order
                .iter()
                .all(|id| self.nodes.contains_key(id) && seen.insert(*id))
    }
}

#[cfg(test)]
#[path = "routing_graph_pdc_tests.rs"]
mod tests;

