//! Send-path delay-compensation block, split out of routing_graph_pdc.rs.
use super::*;

pub(crate) struct SendPdcDelayState {
    requested: AtomicU32,
    left: Box<[f32]>,
    right: Box<[f32]>,
    write: usize,
    valid_samples: u32,
    active: u32,
    previous: u32,
    transition_remaining: u32,
}

#[no_mangle]
pub extern "C" fn hirari_send_pdc_create() -> *mut std::ffi::c_void {
    let state = SendPdcDelayState {
        requested: AtomicU32::new(0),
        left: vec![0.0; SEND_PDC_CAPACITY].into_boxed_slice(),
        right: vec![0.0; SEND_PDC_CAPACITY].into_boxed_slice(),
        write: 0,
        valid_samples: 0,
        active: 0,
        previous: 0,
        transition_remaining: 0,
    };
    Box::into_raw(Box::new(state)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_destroy(handle: *mut std::ffi::c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<SendPdcDelayState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_set_delay(
    handle: *mut std::ffi::c_void,
    samples: u32,
) -> bool {
    if handle.is_null() {
        return false;
    }
    (*std::ptr::addr_of!((*handle.cast::<SendPdcDelayState>()).requested))
        .store(samples.min(SEND_PDC_MAX_DELAY), Ordering::Release);
    true
}

pub(crate) unsafe fn send_pdc_read(state: *mut SendPdcDelayState, delay: u32, right: bool) -> f32 {
    if delay == 0 || delay > (*state).valid_samples {
        return 0.0;
    }
    let read_index = ((*state).write + SEND_PDC_CAPACITY - delay as usize) % SEND_PDC_CAPACITY;
    let channel = if right {
        (*state).right.as_ptr()
    } else {
        (*state).left.as_ptr()
    };
    *channel.add(read_index)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_process(
    handle: *mut std::ffi::c_void,
    input_left: *const f32,
    input_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: u32,
    additional_delay: u32,
    input_gain: f32,
) -> bool {
    if handle.is_null()
        || input_left.is_null()
        || input_right.is_null()
        || output_left.is_null()
        || output_right.is_null()
        || frames == 0
        || frames > SEND_PDC_MAX_DELAY
    {
        return false;
    }
    let state = handle.cast::<SendPdcDelayState>();

    let pdc_delay = (*state).requested.load(Ordering::Acquire);
    let manual_delay = additional_delay.min(SEND_PDC_MAX_DELAY);
    let requested = if pdc_delay > SEND_PDC_MAX_DELAY - manual_delay {
        SEND_PDC_MAX_DELAY
    } else {
        pdc_delay + manual_delay
    };
    if requested != (*state).active {
        (*state).previous = (*state).active;
        (*state).active = requested;
        (*state).transition_remaining = 64;
    }
    let gain = if input_gain.is_finite() {
        input_gain.clamp(0.0, 2.0)
    } else {
        0.0
    };
    for index in 0..frames as usize {
        let sample_left = *input_left.add(index);
        let sample_right = *input_right.add(index);
        let in_left = if sample_left.is_finite() {
            sample_left * gain
        } else {
            0.0
        };
        let in_right = if sample_right.is_finite() {
            sample_right * gain
        } else {
            0.0
        };
        let old_left = if (*state).previous == 0 {
            in_left
        } else {
            send_pdc_read(state, (*state).previous, false)
        };
        let old_right = if (*state).previous == 0 {
            in_right
        } else {
            send_pdc_read(state, (*state).previous, true)
        };
        let new_left = if (*state).active == 0 {
            in_left
        } else {
            send_pdc_read(state, (*state).active, false)
        };
        let new_right = if (*state).active == 0 {
            in_right
        } else {
            send_pdc_read(state, (*state).active, true)
        };
        if (*state).transition_remaining > 0 {
            let progress = (64 - (*state).transition_remaining) as f32 / 63.0;
            output_left
                .add(index)
                .write(old_left + (new_left - old_left) * progress);
            output_right
                .add(index)
                .write(old_right + (new_right - old_right) * progress);
            (*state).transition_remaining -= 1;
        } else {
            output_left.add(index).write(new_left);
            output_right.add(index).write(new_right);
        }
        (*state)
            .left
            .as_mut_ptr()
            .add((*state).write)
            .write(in_left);
        (*state)
            .right
            .as_mut_ptr()
            .add((*state).write)
            .write(in_right);
        (*state).write = ((*state).write + 1) % SEND_PDC_CAPACITY;
        (*state).valid_samples = ((*state).valid_samples + 1).min((SEND_PDC_CAPACITY - 1) as u32);
    }
    true
}

/// Compiles the active engine's signal and processing-dependency edges into
/// the same ascending-node deterministic order used by its realtime publisher.
/// `edge_pairs` is a packed `[source, destination, ...]` array. Returns
/// `usize::MAX` for invalid input or a cycle; an empty graph succeeds with 0.
#[no_mangle]
pub unsafe extern "C" fn hirari_compile_routing_order(
    edge_pairs: *const u32,
    edge_count: usize,
    output: *mut u32,
    output_capacity: usize,
) -> usize {
    const MAX_NODES: usize = 128;
    if output.is_null() || output_capacity < MAX_NODES || (edge_count > 0 && edge_pairs.is_null()) {
        return usize::MAX;
    }
    let Some(edge_values) = edge_count.checked_mul(2) else {
        return usize::MAX;
    };
    let edges = if edge_values == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(edge_pairs, edge_values)
    };
    let mut active = [false; MAX_NODES];
    let mut indegree = [0_u16; MAX_NODES];
    for edge in edges.chunks_exact(2) {
        let source = edge[0] as usize;
        let destination = edge[1] as usize;
        if source >= MAX_NODES || destination >= MAX_NODES || source == destination {
            return usize::MAX;
        }
        active[source] = true;
        active[destination] = true;
        let Some(next) = indegree[destination].checked_add(1) else {
            return usize::MAX;
        };
        indegree[destination] = next;
    }

    let active_count = active.iter().filter(|node| **node).count();
    let mut emitted = [false; MAX_NODES];
    let mut order = [0_u32; MAX_NODES];
    let mut order_count = 0;
    while order_count < active_count {
        let Some(next) =
            (0..MAX_NODES).find(|&node| active[node] && !emitted[node] && indegree[node] == 0)
        else {
            return usize::MAX;
        };
        emitted[next] = true;
        order[order_count] = next as u32;
        order_count += 1;
        for edge in edges.chunks_exact(2) {
            if edge[0] as usize == next {
                let destination = edge[1] as usize;
                let Some(next_degree) = indegree[destination].checked_sub(1) else {
                    return usize::MAX;
                };
                indegree[destination] = next_degree;
            }
        }
    }
    std::slice::from_raw_parts_mut(output, order_count).copy_from_slice(&order[..order_count]);
    order_count
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_would_create_cycle(
    edge_pairs: *const u32,
    edge_count: usize,
    source: u32,
    destination: u32,
) -> bool {
    const MAX_NODES: usize = 128;
    if source as usize >= MAX_NODES
        || destination as usize >= MAX_NODES
        || source == destination
        || (edge_count > 0 && edge_pairs.is_null())
    {
        return true;
    }
    let Some(edge_values) = edge_count.checked_mul(2) else {
        return true;
    };
    let edges = if edge_values == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(edge_pairs, edge_values)
    };
    let mut visited = [false; MAX_NODES];
    let mut stack = [0_u32; MAX_NODES];
    let mut stack_len = 1;
    stack[0] = destination;
    visited[destination as usize] = true;
    while stack_len > 0 {
        stack_len -= 1;
        let node = stack[stack_len];
        if node == source {
            return true;
        }
        for edge in edges.chunks_exact(2) {
            if edge[0] != node || edge[1] as usize >= MAX_NODES {
                continue;
            }
            let next = edge[1] as usize;
            if next == source as usize {
                return true;
            }
            if !visited[next] {
                visited[next] = true;
                stack[stack_len] = edge[1];
                stack_len += 1;
            }
        }
    }
    false
}
