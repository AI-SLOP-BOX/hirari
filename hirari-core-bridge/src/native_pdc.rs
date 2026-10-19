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
        // Per-node compensation for a track's own audio.
        //
        // edge_compensation aligns the signals *arriving* at a node: it delays a
        // shorter incoming path so every contribution meets at that node's
        // input. That is the right model for a shared bus, but it is not what a
        // track delay line needs. A track's delay line acts on that track's own
        // output, so the delay required is the longest path that the track's
        // own signal experiences on the way to master, not the latency it
        // inherits from upstream tracks routed into it.
        //
        // Conflating the two cancels the compensation of every track that sits
        // downstream of a higher-latency track: a 480-sample track routed into a
        // 0-sample track makes the downstream track's accumulated output
        // latency read as 480, so it is published with zero delay even though
        // its own audio reaches master immediately and must be delayed to line
        // up. Compute the own-path latency separately, over the same routing.
        let mut own_path = vec![0u64; ROUTED_NODE_COUNT];
        let mut own_path_valid = vec![false; ROUTED_NODE_COUNT];
        // A node's path depends on its destination's path, and a destination's
        // index is unrelated to its source's: track 5 may route into track 2,
        // just as a track routes into a higher-numbered bus. One fixed iteration
        // order therefore cannot reach reverse topological order for every
        // topology — walking down by index leaves a lower-numbered destination
        // unresolved and silently drops the latency downstream of it. Follow
        // each still-unresolved chain to its end and fill it back to front
        // instead, which is the reverse topological order this needs.
        let mut on_current_path = vec![false; ROUTED_NODE_COUNT];
        for start in 0..ROUTED_NODE_COUNT {
            if own_path_valid[start] {
                continue;
            }
            let mut chain = Vec::new();
            let mut current = start;
            while !own_path_valid[current] && !on_current_path[current] {
                on_current_path[current] = true;
                chain.push(current);
                let destination = self.routing[current] as usize;
                if destination >= ROUTED_NODE_COUNT {
                    break;
                }
                current = destination;
            }
            for &index in chain.iter().rev() {
                let destination = self.routing[index] as usize;
                let downstream = if destination >= ROUTED_NODE_COUNT {
                    0
                } else if own_path_valid[destination] {
                    own_path[destination]
                } else {
                    // A routing cycle: the shared solver rejects this graph and
                    // the caller discards the solution, so leave this node at
                    // zero rather than guess.
                    0
                };
                let own = if index < MAX_TRACKS {
                    u64::from(self.track_latencies[index])
                } else {
                    u64::from(self.bus_latencies[index - MAX_TRACKS])
                };
                own_path[index] = own.saturating_add(downstream);
                own_path_valid[index] = true;
                on_current_path[index] = false;
            }
        }

        let mut node_own_compensation = vec![0u32; ROUTED_NODE_COUNT];
        let mut maximum_own_path = 0u64;
        for index in 0..ROUTED_NODE_COUNT {
            maximum_own_path = maximum_own_path.max(own_path[index]);
        }
        for index in 0..ROUTED_NODE_COUNT {
            node_own_compensation[index] =
                (maximum_own_path.saturating_sub(own_path[index])).min(u64::from(u32::MAX)) as u32;
        }

        valid.then_some(PdcSolution {
            edge_sources,
            edge_destinations,
            edge_compensations,
            global_latency,
            node_own_compensation,
        })
    }
}

struct PdcSolution {
    edge_sources: Vec<u32>,
    edge_destinations: Vec<u32>,
    edge_compensations: Vec<u32>,
    global_latency: u32,
    /// Per-node delay for a track's own output, indexed by routed node id.
    node_own_compensation: Vec<u32>,
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
                solution.node_own_compensation[track],
                Ordering::Relaxed,
            );
        }
        for bus in 0..MAX_BUSES {
            let node = MAX_TRACKS + bus;
            self.published.bus_offsets[write][bus].store(
                solution.node_own_compensation[node],
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
                != solution.node_own_compensation[track]
            {
                return false;
            }
        }
        for bus in 0..MAX_BUSES {
            let node = MAX_TRACKS + bus;
            if self.published.bus_offsets[active][bus].load(Ordering::Acquire)
                != solution.node_own_compensation[node]
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

    #[test]
    fn own_path_walk_covers_routes_into_lower_indexed_tracks() {
        // Regression: a route whose destination has a *lower* index than its
        // source (track 2 feeding track 1) still has to contribute its
        // downstream latency. Walking the configuration in descending index
        // order saw track 1 as still unknown, read track 2's path as zero, and
        // published a 100-sample delay for a track that already carries track
        // 1's full 100-sample latency — double-counting the same signal.
        let pdc = NativePdcManager::new();
        assert!(pdc.bind_control_thread());
        pdc.reset_for_project();
        assert!(pdc.set_latency(false, 1, 100));
        assert!(pdc.set_latency(false, 2, 0));
        assert!(pdc.set_track_destination(2, 1));
        pdc.recalculate();

        assert_eq!(pdc.get_track_offset_for_test(1), 0);
        assert_eq!(pdc.get_track_offset_for_test(2), 0);
        assert_eq!(pdc.published.max_global.load(Ordering::Acquire), 100);
        assert!(pdc.audit());
    }

    impl NativePdcManager {
        fn get_track_offset_for_test(&self, track: usize) -> u32 {
            let active = self.published.active_buffer.load(Ordering::Acquire) as usize;
            self.published.track_offsets[active][track].load(Ordering::Acquire)
        }
    }
}
