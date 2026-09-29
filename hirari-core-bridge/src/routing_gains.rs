//! Atomic gain and send-state tables used by routing callbacks.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const MAX_ROUTING_NODES: u32 = 128;
const SLOT_COUNT: usize = MAX_ROUTING_NODES as usize * MAX_ROUTING_NODES as usize;

pub struct RoutingGainState {
    gains: Box<[AtomicU32]>,
    send_gains: Box<[AtomicU32]>,
    send_pre_fader: Box<[AtomicBool]>,
    send_connected: Box<[AtomicBool]>,
}

impl Default for RoutingGainState {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingGainState {
    pub fn new() -> Self {
        Self {
            gains: (0..SLOT_COUNT)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            send_gains: (0..SLOT_COUNT)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            send_pre_fader: (0..SLOT_COUNT)
                .map(|_| AtomicBool::new(false))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            send_connected: (0..SLOT_COUNT)
                .map(|_| AtomicBool::new(false))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    fn index(source: u32, destination: u32) -> Option<usize> {
        if source >= MAX_ROUTING_NODES || destination >= MAX_ROUTING_NODES {
            None
        } else {
            Some(source as usize * MAX_ROUTING_NODES as usize + destination as usize)
        }
    }

    pub fn reset(&self) {
        for gain in self.gains.iter().chain(self.send_gains.iter()) {
            gain.store(0.0_f32.to_bits(), Ordering::Release);
        }
        for flag in self.send_pre_fader.iter().chain(self.send_connected.iter()) {
            flag.store(false, Ordering::Release);
        }
    }

    pub fn set_route_gain(&self, source: u32, destination: u32, gain: f32) -> bool {
        let Some(index) = Self::index(source, destination) else {
            return false;
        };
        self.gains[index].store(gain.to_bits(), Ordering::Release);
        true
    }

    pub fn set_send(&self, source: u32, destination: u32, gain: f32, pre_fader: bool) -> bool {
        let Some(index) = Self::index(source, destination) else {
            return false;
        };
        self.send_gains[index].store(gain.to_bits(), Ordering::Release);
        self.send_pre_fader[index].store(pre_fader, Ordering::Release);
        self.send_connected[index].store(true, Ordering::Release);
        true
    }

    pub fn clear_route(&self, source: u32, destination: u32) -> bool {
        self.set_route_gain(source, destination, 0.0)
    }

    pub fn clear_send(&self, source: u32, destination: u32) -> bool {
        let Some(index) = Self::index(source, destination) else {
            return false;
        };
        self.send_gains[index].store(0.0_f32.to_bits(), Ordering::Release);
        self.send_pre_fader[index].store(false, Ordering::Release);
        self.send_connected[index].store(false, Ordering::Release);
        true
    }

    pub fn route_gain(&self, source: u32, destination: u32) -> f32 {
        Self::index(source, destination)
            .map(|index| f32::from_bits(self.gains[index].load(Ordering::Acquire)))
            .unwrap_or(0.0)
    }

    pub fn send_gain(&self, source: u32, destination: u32) -> f32 {
        Self::index(source, destination)
            .map(|index| f32::from_bits(self.send_gains[index].load(Ordering::Acquire)))
            .unwrap_or(0.0)
    }

    pub fn has_send(&self, source: u32, destination: u32) -> bool {
        Self::index(source, destination)
            .is_some_and(|index| self.send_connected[index].load(Ordering::Acquire))
    }

    pub fn send_is_pre_fader(&self, source: u32, destination: u32) -> bool {
        Self::index(source, destination)
            .is_some_and(|index| self.send_pre_fader[index].load(Ordering::Acquire))
    }
}

pub(crate) unsafe fn state<'a>(handle: *const c_void) -> Option<&'a RoutingGainState> {
    handle.cast::<RoutingGainState>().as_ref()
}

#[no_mangle]
pub extern "C" fn hirari_routing_gains_create() -> *mut c_void {
    Box::into_raw(Box::new(RoutingGainState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<RoutingGainState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_reset(handle: *const c_void) {
    if let Some(state) = state(handle) {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_set_route(
    handle: *const c_void,
    source: u32,
    destination: u32,
    gain: f32,
) -> bool {
    state(handle).is_some_and(|state| state.set_route_gain(source, destination, gain))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_set_send(
    handle: *const c_void,
    source: u32,
    destination: u32,
    gain: f32,
    pre_fader: bool,
) -> bool {
    state(handle).is_some_and(|state| state.set_send(source, destination, gain, pre_fader))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_clear_route(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    state(handle).is_some_and(|state| state.clear_route(source, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_clear_send(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    state(handle).is_some_and(|state| state.clear_send(source, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_route(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    state(handle).map_or(0.0, |state| state.route_gain(source, destination))
}

/// Decides whether a track contributes to the active render target. Uses only
/// fixed-size stack storage so it is safe on the audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_should_process_node(
    handle: *const c_void,
    source: u32,
    offline_target_active: bool,
    offline_target: u32,
    any_solo: bool,
    source_solo: bool,
) -> bool {
    if !offline_target_active {
        return !any_solo || source_solo;
    }
    if offline_target == 0 {
        return true;
    }
    if source == offline_target {
        return true;
    }
    let Some(state) = state(handle) else {
        return false;
    };
    if source >= MAX_ROUTING_NODES || offline_target >= MAX_ROUTING_NODES {
        return false;
    }

    let mut queue = [0_u32; MAX_ROUTING_NODES as usize];
    let mut visited = [false; MAX_ROUTING_NODES as usize];
    let (mut head, mut tail) = (0_usize, 1_usize);
    queue[0] = source;
    visited[source as usize] = true;
    while head < tail {
        let current = queue[head];
        head += 1;
        for candidate in 0..MAX_ROUTING_NODES {
            let index = candidate as usize;
            if state.route_gain(current, candidate) <= 0.0 || visited[index] {
                continue;
            }
            if candidate == offline_target {
                return true;
            }
            visited[index] = true;
            queue[tail] = candidate;
            tail += 1;
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_send(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> f32 {
    state(handle).map_or(0.0, |state| state.send_gain(source, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_has_send(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    state(handle).is_some_and(|state| state.has_send(source, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_routing_gains_send_pre_fader(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> bool {
    state(handle).is_some_and(|state| state.send_is_pre_fader(source, destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_and_send_gains_have_independent_atomic_tables() {
        let state = RoutingGainState::new();
        assert!(state.set_route_gain(4, 9, 0.75));
        assert!(state.set_send(4, 9, 0.25, true));
        assert_eq!(state.route_gain(4, 9), 0.75);
        assert_eq!(state.send_gain(4, 9), 0.25);
        assert!(state.has_send(4, 9));
        assert!(state.send_is_pre_fader(4, 9));
        assert!(state.clear_send(4, 9));
        assert!(!state.has_send(4, 9));
        assert_eq!(state.send_gain(4, 9), 0.0);
        assert_eq!(state.route_gain(4, 9), 0.75);
    }

    #[test]
    fn reset_and_bounds_are_explicit() {
        let state = RoutingGainState::new();
        assert!(!state.set_route_gain(128, 0, 1.0));
        assert!(!state.set_send(0, 128, 1.0, false));
        assert!(state.set_route_gain(0, 1, 0.5));
        assert!(state.set_send(0, 1, 0.25, false));
        state.reset();
        assert_eq!(state.route_gain(0, 1), 0.0);
        assert_eq!(state.send_gain(0, 1), 0.0);
        assert!(!state.has_send(0, 1));
    }
}
