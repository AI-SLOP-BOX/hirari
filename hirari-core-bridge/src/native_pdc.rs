//! Native-compatible PDC manager state. Control configuration and graph
//! solving are owned by Rust; callback readers use only the published atomics.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;

const MAX_TRACKS: usize = 512;
const MAX_BUSES: usize = 128;
const MASTER_ID: u32 = u32::MAX;
const ROUTED_NODE_COUNT: usize = MAX_TRACKS + MAX_BUSES;
const SEND_OFFSET_COUNT: usize = MAX_TRACKS * MAX_TRACKS;

struct PdcConfiguration {
    routing: Vec<u32>,
    track_latencies: Vec<u32>,
    bus_latencies: Vec<u32>,
    send_routes: Vec<(u32, u32)>,
}

impl PdcConfiguration {
    fn new() -> Self {
        Self {
            routing: vec![MASTER_ID; ROUTED_NODE_COUNT],
            track_latencies: vec![0; MAX_TRACKS],
            bus_latencies: vec![0; MAX_BUSES],
            send_routes: Vec::new(),
        }
    }

    fn solve(&self) -> Option<PdcSolution> {
        let mut node_ids = (0..ROUTED_NODE_COUNT as u32).collect::<Vec<_>>();
        let mut node_latencies = self
            .track_latencies
            .iter()
            .copied()
            .chain(self.bus_latencies.iter().copied())
            .collect::<Vec<_>>();
        node_ids.push(MASTER_ID);
        node_latencies.push(0);

        let mut edge_sources = Vec::with_capacity(ROUTED_NODE_COUNT + self.send_routes.len());
        let mut edge_destinations = Vec::with_capacity(ROUTED_NODE_COUNT + self.send_routes.len());
        for source in 0..ROUTED_NODE_COUNT {
            let destination = self.routing[source];
            if destination == MASTER_ID || (destination as usize) < ROUTED_NODE_COUNT {
                edge_sources.push(source as u32);
                edge_destinations.push(destination);
            }
        }
        for (source, destination) in &self.send_routes {
            if self.routing[*source as usize] != *destination {
                edge_sources.push(*source);
                edge_destinations.push(*destination);
            }
        }

        let mut edge_compensations = vec![0; edge_sources.len()];
        let mut global_latency = 0;
        // SAFETY: every pointer refers to a live vector of the paired length;
        // output vectors are sized for every graph node and edge.
        let valid = unsafe {
            crate::pdc_manager::hirari_pdc_solve_engine_graph(
                node_ids.as_ptr(),
                node_latencies.as_ptr(),
                node_ids.len(),
                edge_sources.as_ptr(),
                edge_destinations.as_ptr(),
                edge_sources.len(),
                edge_compensations.as_mut_ptr(),
                edge_compensations.len(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                &mut global_latency,
            )
        };
        valid.then_some(PdcSolution {
            edge_sources,
            edge_destinations,
            edge_compensations,
            global_latency,
        })
    }
}

struct PdcSolution {
    edge_sources: Vec<u32>,
    edge_destinations: Vec<u32>,
    edge_compensations: Vec<u32>,
    global_latency: u32,
}

impl PdcSolution {
    fn compensation_for(&self, source: u32, destination: u32) -> u32 {
        self.edge_sources
            .iter()
            .zip(&self.edge_destinations)
            .position(|(edge_source, edge_destination)| {
                *edge_source == source && *edge_destination == destination
            })
            .and_then(|index| self.edge_compensations.get(index).copied())
            .unwrap_or(0)
    }
}

struct PublishedPdc {
    track_offsets: [Box<[AtomicU32]>; 2],
    bus_offsets: [Box<[AtomicU32]>; 2],
    send_offsets: [Box<[AtomicU32]>; 2],
    active_buffer: AtomicU32,
    max_global: AtomicU32,
    cycle_detected: AtomicBool,
}

impl PublishedPdc {
    fn new() -> Self {
        Self {
            track_offsets: std::array::from_fn(|_| {
                (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            }),
            bus_offsets: std::array::from_fn(|_| {
                (0..MAX_BUSES)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            }),
            send_offsets: std::array::from_fn(|_| {
                (0..SEND_OFFSET_COUNT)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            }),
            active_buffer: AtomicU32::new(0),
            max_global: AtomicU32::new(0),
            cycle_detected: AtomicBool::new(false),
        }
    }

    fn clear_buffer(&self, index: usize) {
        for offset in self.track_offsets[index].iter() {
            offset.store(0, Ordering::Relaxed);
        }
        for offset in self.bus_offsets[index].iter() {
            offset.store(0, Ordering::Relaxed);
        }
        for offset in self.send_offsets[index].iter() {
            offset.store(0, Ordering::Relaxed);
        }
    }
}

struct NativePdcManager {
    configuration: Mutex<PdcConfiguration>,
    control_thread: Mutex<Option<ThreadId>>,
    dirty: AtomicBool,
    configuration_generation: AtomicU64,
    low_latency_mode: AtomicBool,
    published: PublishedPdc,
}

impl NativePdcManager {
    fn new() -> Self {
        Self {
            configuration: Mutex::new(PdcConfiguration::new()),
            control_thread: Mutex::new(None),
            dirty: AtomicBool::new(true),
            configuration_generation: AtomicU64::new(0),
            low_latency_mode: AtomicBool::new(false),
            published: PublishedPdc::new(),
        }
    }

    fn bind_control_thread(&self) -> bool {
        let current = std::thread::current().id();
        let Ok(mut owner) = self.control_thread.lock() else {
            return false;
        };
        match owner.as_ref() {
            Some(bound) => *bound == current,
            None => {
                *owner = Some(current);
                true
            }
        }
    }

    fn control_thread_allowed(&self) -> bool {
        self.control_thread
            .lock()
            .ok()
            .and_then(|owner| {
                owner
                    .as_ref()
                    .map(|bound| *bound == std::thread::current().id())
            })
            .unwrap_or(false)
    }

    fn mark_dirty_locked(&self) {
        self.configuration_generation.fetch_add(1, Ordering::AcqRel);
        self.dirty.store(true, Ordering::Release);
    }

    fn set_track_destination(&self, track: u32, destination: u32) -> bool {
        if track as usize >= MAX_TRACKS || !self.control_thread_allowed() {
            return false;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return false;
        };
        configuration.routing[track as usize] = destination;
        self.mark_dirty_locked();
        true
    }

    fn set_bus_destination(&self, bus: u32, destination: u32) -> bool {
        if bus as usize >= MAX_BUSES || !self.control_thread_allowed() {
            return false;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return false;
        };
        configuration.routing[MAX_TRACKS + bus as usize] = destination;
        self.mark_dirty_locked();
        true
    }

    fn set_send_route(&self, source: u32, destination: u32, enabled: bool) -> bool {
        if source as usize >= MAX_TRACKS
            || destination as usize >= MAX_TRACKS
            || source == destination
            || !self.control_thread_allowed()
        {
            return false;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return false;
        };
        let route = (source, destination);
        match (
            enabled,
            configuration
                .send_routes
                .iter()
                .position(|edge| *edge == route),
        ) {
            (true, None) => configuration.send_routes.push(route),
            (false, Some(index)) => {
                configuration.send_routes.remove(index);
            }
            _ => return true,
        }
        self.mark_dirty_locked();
        true
    }

    fn set_latency(&self, is_bus: bool, index: u32, samples: u32) -> bool {
        let limit = if is_bus { MAX_BUSES } else { MAX_TRACKS };
        if index as usize >= limit || !self.control_thread_allowed() {
            return false;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return false;
        };
        if is_bus {
            configuration.bus_latencies[index as usize] = samples;
        } else {
            configuration.track_latencies[index as usize] = samples;
        }
        self.mark_dirty_locked();
        true
    }

    fn set_low_latency_mode(&self, active: bool) -> bool {
        if !self.control_thread_allowed() {
            return false;
        }
        let Ok(_configuration) = self.configuration.lock() else {
            return false;
        };
        self.low_latency_mode.store(active, Ordering::Release);
        self.mark_dirty_locked();
        true
    }

    fn clear_configuration(&self) -> bool {
        if !self.control_thread_allowed() {
            return false;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return false;
        };
        configuration.routing.fill(MASTER_ID);
        configuration.track_latencies.fill(0);
        configuration.bus_latencies.fill(0);
        configuration.send_routes.clear();
        self.mark_dirty_locked();
        true
    }

    fn reset_for_project(&self) {
        if !self.control_thread_allowed() {
            return;
        }
        let Ok(mut configuration) = self.configuration.lock() else {
            return;
        };
        configuration.routing.fill(MASTER_ID);
        configuration.track_latencies.fill(0);
        configuration.bus_latencies.fill(0);
        configuration.send_routes.clear();
        let active = self.published.active_buffer.load(Ordering::Acquire) as usize;
        let write = 1 - active;
        self.published.clear_buffer(write);
        self.published
            .active_buffer
            .store(write as u32, Ordering::Release);
        self.published.max_global.store(0, Ordering::Release);
        self.published
            .cycle_detected
            .store(false, Ordering::Release);
        self.mark_dirty_locked();
    }

    fn recalculate(&self) {
        if !self.control_thread_allowed() {
            return;
        }
        let Ok(configuration) = self.configuration.lock() else {
            return;
        };
        if !self.dirty.load(Ordering::Acquire) {
            return;
        }
        let active = self.published.active_buffer.load(Ordering::Relaxed) as usize;
        let write = 1 - active;
        if self.low_latency_mode.load(Ordering::Acquire) {
            self.published.clear_buffer(write);
            self.published
                .cycle_detected
                .store(false, Ordering::Release);
            self.published
                .active_buffer
                .store(write as u32, Ordering::Release);
            self.published.max_global.store(0, Ordering::Release);
            self.dirty.store(false, Ordering::Release);
            return;
        }
        let Some(solution) = configuration.solve() else {
            self.published.cycle_detected.store(true, Ordering::Release);
            self.dirty.store(false, Ordering::Release);
            return;
        };
        for track in 0..MAX_TRACKS {
            self.published.track_offsets[write][track].store(
                solution.compensation_for(track as u32, configuration.routing[track]),
                Ordering::Relaxed,
            );
        }
        for bus in 0..MAX_BUSES {
            let node = MAX_TRACKS + bus;
            self.published.bus_offsets[write][bus].store(
                solution.compensation_for(node as u32, configuration.routing[node]),
                Ordering::Relaxed,
            );
        }
        for offset in self.published.send_offsets[write].iter() {
            offset.store(0, Ordering::Relaxed);
        }
        for (source, destination) in &configuration.send_routes {
            let index = *source as usize * MAX_TRACKS + *destination as usize;
            self.published.send_offsets[write][index].store(
                solution.compensation_for(*source, *destination),
                Ordering::Relaxed,
            );
        }
        self.published
            .cycle_detected
            .store(false, Ordering::Release);
        self.published
            .active_buffer
            .store(write as u32, Ordering::Release);
        self.published
            .max_global
            .store(solution.global_latency, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
    }

    fn audit(&self) -> bool {
        if !self.control_thread_allowed() {
            return false;
        }
        let Ok(configuration) = self.configuration.lock() else {
            return false;
        };
        let Some(solution) = configuration.solve() else {
            return self.published.cycle_detected.load(Ordering::Acquire);
        };
        if solution.global_latency != self.published.max_global.load(Ordering::Acquire) {
            return false;
        }
        let active = self.published.active_buffer.load(Ordering::Acquire) as usize;
        for track in 0..MAX_TRACKS {
            if self.published.track_offsets[active][track].load(Ordering::Acquire)
                != solution.compensation_for(track as u32, configuration.routing[track])
            {
                return false;
            }
        }
        for bus in 0..MAX_BUSES {
            let node = MAX_TRACKS + bus;
            if self.published.bus_offsets[active][bus].load(Ordering::Acquire)
                != solution.compensation_for(node as u32, configuration.routing[node])
            {
                return false;
            }
        }
        for (source, destination) in &configuration.send_routes {
            let index = *source as usize * MAX_TRACKS + *destination as usize;
            if self.published.send_offsets[active][index].load(Ordering::Acquire)
                != solution.compensation_for(*source, *destination)
            {
                return false;
            }
        }
        true
    }
}

unsafe fn state(handle: *const c_void) -> Option<&'static NativePdcManager> {
    if handle.is_null() {
        None
    } else {
        // SAFETY: native handles are allocated and released by this module.
        Some(unsafe { &*handle.cast::<NativePdcManager>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_pdc_create() -> *mut c_void {
    Box::into_raw(Box::new(NativePdcManager::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: handle was returned by `hirari_native_pdc_create`.
        drop(unsafe { Box::from_raw(handle.cast::<NativePdcManager>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_bind_control_thread(handle: *const c_void) -> bool {
    unsafe { state(handle) }.is_some_and(NativePdcManager::bind_control_thread)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_track_destination(
    handle: *const c_void,
    track: u32,
    destination: u32,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_track_destination(track, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_bus_destination(
    handle: *const c_void,
    bus: u32,
    destination: u32,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_bus_destination(bus, destination))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_send_route(
    handle: *const c_void,
    source: u32,
    destination: u32,
    enabled: bool,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_send_route(source, destination, enabled))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_track_latency(
    handle: *const c_void,
    track: u32,
    samples: u32,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_latency(false, track, samples))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_bus_latency(
    handle: *const c_void,
    bus: u32,
    samples: u32,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_latency(true, bus, samples))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_set_low_latency_mode(
    handle: *const c_void,
    active: bool,
) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.set_low_latency_mode(active))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_clear_configuration(handle: *const c_void) -> bool {
    unsafe { state(handle) }.is_some_and(NativePdcManager::clear_configuration)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_mark_dirty(handle: *const c_void) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if !state.control_thread_allowed() {
        return false;
    }
    let Ok(_configuration) = state.configuration.lock() else {
        return false;
    };
    state.mark_dirty_locked();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_reset_for_project(handle: *const c_void) {
    if let Some(state) = unsafe { state(handle) } {
        state.reset_for_project();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_recalculate(handle: *const c_void) {
    if let Some(state) = unsafe { state(handle) } {
        state.recalculate();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_audit(handle: *const c_void) -> bool {
    unsafe { state(handle) }.is_some_and(NativePdcManager::audit)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_get_track_offset(
    handle: *const c_void,
    track: u32,
) -> u32 {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    if track as usize >= MAX_TRACKS {
        return 0;
    }
    let active = state.published.active_buffer.load(Ordering::Acquire) as usize;
    state.published.track_offsets[active][track as usize].load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_get_bus_offset(handle: *const c_void, bus: u32) -> u32 {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    if bus as usize >= MAX_BUSES {
        return 0;
    }
    let active = state.published.active_buffer.load(Ordering::Acquire) as usize;
    state.published.bus_offsets[active][bus as usize].load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_get_send_offset(
    handle: *const c_void,
    source: u32,
    destination: u32,
) -> u32 {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    if source as usize >= MAX_TRACKS || destination as usize >= MAX_TRACKS {
        return 0;
    }
    let active = state.published.active_buffer.load(Ordering::Acquire) as usize;
    state.published.send_offsets[active][source as usize * MAX_TRACKS + destination as usize]
        .load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_get_global_latency(handle: *const c_void) -> u32 {
    unsafe { state(handle) }
        .map(|state| state.published.max_global.load(Ordering::Acquire))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_has_cycle(handle: *const c_void) -> bool {
    unsafe { state(handle) }
        .is_some_and(|state| state.published.cycle_detected.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_low_latency_mode(handle: *const c_void) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.low_latency_mode.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_pdc_configuration_generation(handle: *const c_void) -> u64 {
    unsafe { state(handle) }
        .map(|state| state.configuration_generation.load(Ordering::Acquire))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_pdc_control_owner_and_double_buffered_offsets() {
        let pdc = NativePdcManager::new();
        assert!(!pdc.set_latency(false, 1, 480));
        assert!(pdc.bind_control_thread());
        assert!(pdc.set_latency(false, 1, 480));
        assert!(pdc.set_latency(false, 2, 0));
        assert!(pdc.set_track_destination(1, 2));
        pdc.recalculate();
        assert!(!pdc.dirty.load(Ordering::Acquire));
        assert!(pdc.audit());
        assert_eq!(pdc.published.max_global.load(Ordering::Acquire), 480);
    }

    #[test]
    fn reset_and_low_latency_publish_zero_offsets_without_touching_active_data() {
        let pdc = NativePdcManager::new();
        assert!(pdc.bind_control_thread());
        assert!(pdc.set_latency(false, 1, 256));
        pdc.recalculate();
        assert!(pdc.set_low_latency_mode(true));
        pdc.recalculate();
        assert_eq!(pdc.get_track_offset_for_test(1), 0);
        pdc.reset_for_project();
        assert_eq!(pdc.published.max_global.load(Ordering::Acquire), 0);
        assert!(pdc.dirty.load(Ordering::Acquire));
    }

    impl NativePdcManager {
        fn get_track_offset_for_test(&self, track: usize) -> u32 {
            let active = self.published.active_buffer.load(Ordering::Acquire) as usize;
            self.published.track_offsets[active][track].load(Ordering::Acquire)
        }
    }
}
