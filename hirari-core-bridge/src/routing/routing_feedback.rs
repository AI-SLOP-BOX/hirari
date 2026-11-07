//! Fixed-capacity feedback routing state and block kernels.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MAX_NODES: u32 = 128;
const MAX_CONNECTIONS: usize = 16;
const MAX_SAMPLES: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeedbackConnection {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
}

struct FeedbackEdge {
    source: AtomicU32,
    destination: AtomicU32,
    gain_bits: AtomicU32,
    active: AtomicBool,
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
}

impl FeedbackEdge {
    fn new() -> Self {
        Self {
            source: AtomicU32::new(0),
            destination: AtomicU32::new(0),
            gain_bits: AtomicU32::new(1.0_f32.to_bits()),
            active: AtomicBool::new(false),
            left: (0..MAX_SAMPLES)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            right: (0..MAX_SAMPLES)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }

    fn clear_audio(&self) {
        for sample in self.left.iter().chain(self.right.iter()) {
            sample.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }
}

pub struct FeedbackRoutingState {
    edges: [FeedbackEdge; MAX_CONNECTIONS],
}

impl Default for FeedbackRoutingState {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedbackRoutingState {
    pub fn new() -> Self {
        Self {
            edges: std::array::from_fn(|_| FeedbackEdge::new()),
        }
    }

    pub fn reset(&self) {
        for edge in &self.edges {
            edge.active.store(false, Ordering::Release);
        }
    }

    pub fn add(&self, source: u32, destination: u32, gain: f32) -> bool {
        if source >= MAX_NODES
            || destination >= MAX_NODES
            || source == destination
            || !gain.is_finite()
        {
            return false;
        }
        let gain = gain.clamp(0.0, 2.0);
        for edge in &self.edges {
            if edge.active.load(Ordering::Acquire)
                && edge.source.load(Ordering::Relaxed) == source
                && edge.destination.load(Ordering::Relaxed) == destination
            {
                edge.active.store(false, Ordering::Release);
                edge.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
                edge.clear_audio();
                edge.active.store(true, Ordering::Release);
                return true;
            }
        }
        for edge in &self.edges {
            if !edge.active.load(Ordering::Acquire) {
                edge.source.store(source, Ordering::Relaxed);
                edge.destination.store(destination, Ordering::Relaxed);
                edge.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
                edge.clear_audio();
                edge.active.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }

    pub fn remove(&self, source: u32, destination: u32) {
        for edge in &self.edges {
            if edge.active.load(Ordering::Acquire)
                && edge.source.load(Ordering::Relaxed) == source
                && edge.destination.load(Ordering::Relaxed) == destination
            {
                edge.active.store(false, Ordering::Release);
            }
        }
    }

    pub fn remove_for_node(&self, node: u32, output: &mut [FeedbackConnection]) -> usize {
        if node >= MAX_NODES {
            return 0;
        }
        let mut count = 0;
        for edge in &self.edges {
            if !edge.active.load(Ordering::Acquire) {
                continue;
            }
            let source = edge.source.load(Ordering::Relaxed);
            let destination = edge.destination.load(Ordering::Relaxed);
            if source != node && destination != node {
                continue;
            }
            let gain = edge.gain();
            edge.active.store(false, Ordering::Release);
            if count < output.len() {
                output[count] = FeedbackConnection {
                    source_id: source,
                    destination_id: destination,
                    gain,
                };
                count += 1;
            }
        }
        count
    }

    pub fn gain(&self, source: u32, destination: u32) -> f32 {
        if source >= MAX_NODES || destination >= MAX_NODES {
            return -1.0;
        }
        self.edges
            .iter()
            .find(|edge| {
                edge.active.load(Ordering::Acquire)
                    && edge.source.load(Ordering::Relaxed) == source
                    && edge.destination.load(Ordering::Relaxed) == destination
            })
            .map_or(-1.0, FeedbackEdge::gain)
    }

    pub fn copy_connections(&self, output: &mut [FeedbackConnection]) -> usize {
        let mut count = 0;
        for edge in &self.edges {
            if !edge.active.load(Ordering::Acquire) {
                continue;
            }
            if count == output.len() {
                break;
            }
            output[count] = FeedbackConnection {
                source_id: edge.source.load(Ordering::Relaxed),
                destination_id: edge.destination.load(Ordering::Relaxed),
                gain: edge.gain(),
            };
            count += 1;
        }
        count
    }

    pub fn inject(&self, destination: u32, left: &mut [f32], right: &mut [f32]) {
        if destination >= MAX_NODES
            || left.is_empty()
            || left.len() != right.len()
            || left.len() > MAX_SAMPLES
        {
            return;
        }
        for edge in &self.edges {
            if !edge.active.load(Ordering::Acquire)
                || edge.destination.load(Ordering::Relaxed) != destination
            {
                continue;
            }
            let gain = edge.gain();
            for frame in 0..left.len() {
                left[frame] += f32::from_bits(edge.left[frame].load(Ordering::Relaxed)) * gain;
                right[frame] += f32::from_bits(edge.right[frame].load(Ordering::Relaxed)) * gain;
            }
        }
    }

    pub fn capture(&self, source: u32, left: &[f32], right: &[f32]) {
        if source >= MAX_NODES
            || left.is_empty()
            || left.len() != right.len()
            || left.len() > MAX_SAMPLES
        {
            return;
        }
        for edge in self.edges.iter() {
            if !edge.active.load(Ordering::Acquire) || edge.source.load(Ordering::Relaxed) != source
            {
                continue;
            }
            for frame in 0..left.len() {
                edge.left[frame].store(left[frame].to_bits(), Ordering::Relaxed);
                edge.right[frame].store(right[frame].to_bits(), Ordering::Relaxed);
            }
            for frame in left.len()..MAX_SAMPLES {
                edge.left[frame].store(0.0_f32.to_bits(), Ordering::Relaxed);
                edge.right[frame].store(0.0_f32.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

unsafe fn state<'a>(handle: *const c_void) -> Option<&'a FeedbackRoutingState> {
    handle.cast::<FeedbackRoutingState>().as_ref()
}

#[no_mangle]
pub extern "C" fn hirari_routing_feedback_create() -> *mut c_void {
    Box::into_raw(Box::new(FeedbackRoutingState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<FeedbackRoutingState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_reset(handle: *const c_void) {
    if let Some(state) = state(handle) {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_add(
    handle: *const c_void,
    source: u32,
    destination: u32,
    gain: f32,
) -> bool {
    state(handle).is_some_and(|state| state.add(source, destination, gain))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_remove(
    handle: *const c_void,
    source: u32,
    destination: u32,
) {
    if let Some(state) = state(handle) {
        state.remove(source, destination);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_remove_node(
    handle: *const c_void,
    node: u32,
    output: *mut FeedbackConnection,
    capacity: usize,
) -> usize {
    let (Some(state), Some(output)) = (state(handle), output.as_mut()) else {
        return 0;
    };
    let output = std::slice::from_raw_parts_mut(output, capacity);
    state.remove_for_node(node, output)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_gain(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    state(handle).map_or(-1.0, |state| state.gain(source, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_copy(
    handle: *const c_void,
    output: *mut FeedbackConnection,
    capacity: usize,
) -> usize {
    let (Some(state), Some(output)) = (state(handle), output.as_mut()) else {
        return 0;
    };
    state.copy_connections(std::slice::from_raw_parts_mut(output, capacity))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_inject(
    handle: *const c_void,
    destination: u32,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if left.is_null() || right.is_null() || frames == 0 || frames as usize > MAX_SAMPLES {
        return;
    }
    if let Some(state) = state(handle) {
        state.inject(
            destination,
            std::slice::from_raw_parts_mut(left, frames as usize),
            std::slice::from_raw_parts_mut(right, frames as usize),
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_feedback_capture(
    handle: *const c_void,
    source: u32,
    left: *const f32,
    right: *const f32,
    frames: u32,
) {
    if left.is_null() || right.is_null() || frames == 0 || frames as usize > MAX_SAMPLES {
        return;
    }
    if let Some(state) = state(handle) {
        state.capture(
            source,
            std::slice::from_raw_parts(left, frames as usize),
            std::slice::from_raw_parts(right, frames as usize),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_and_injects_block_with_gain_and_zeroes_old_tail() {
        let state = FeedbackRoutingState::new();
        assert!(state.add(2, 7, 0.5));
        state.capture(2, &[1.0, -0.5], &[0.25, -1.0]);
        let mut left = [0.0; 4];
        let mut right = [0.0; 4];
        state.inject(7, &mut left, &mut right);
        assert_eq!(left, [0.5, -0.25, 0.0, 0.0]);
        assert_eq!(right, [0.125, -0.5, 0.0, 0.0]);
    }

    #[test]
    fn updates_existing_connection_and_reports_removed_edges() {
        let state = FeedbackRoutingState::new();
        assert!(state.add(3, 9, 0.25));
        assert!(state.add(3, 9, 1.5));
        assert_eq!(state.gain(3, 9), 1.5);
        let mut snapshot = [FeedbackConnection::default(); MAX_CONNECTIONS];
        assert_eq!(state.copy_connections(&mut snapshot), 1);
        assert_eq!(
            snapshot[0],
            FeedbackConnection {
                source_id: 3,
                destination_id: 9,
                gain: 1.5
            }
        );
        assert_eq!(state.remove_for_node(9, &mut snapshot), 1);
        assert_eq!(state.gain(3, 9), -1.0);
    }

    #[test]
    fn rejects_invalid_edges_and_enforces_fixed_capacity() {
        let state = FeedbackRoutingState::new();
        assert!(!state.add(1, 1, 0.5));
        assert!(!state.add(0, 1, f32::NAN));
        for source in 0..MAX_CONNECTIONS as u32 {
            assert!(state.add(source, 100 + source, 3.0));
        }
        assert!(!state.add(99, 100, 0.5));
        assert_eq!(state.gain(0, 100), 2.0);
        assert_eq!(state.gain(128, 1), -1.0);
    }
}
