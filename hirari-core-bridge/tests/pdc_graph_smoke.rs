//! Ported 1:1 from `tests/pdc_graph_smoke.cpp`.
//!
//! The C++ file stopped compiling in `442b899` ("Remove the unused PDCManager
//! singleton"): it called `PDCManager::getInstance()`, which that commit
//! deleted. It was only referenced by `scripts/test_pdc_graph.sh`, which
//! therefore never built, and no CI job ran either — so the expectations below
//! were observed nowhere.
//!
//! What happened to the assertions: `PDCGraphSolver` and `RoutingGraphPDC`
//! were pure C++ at `4e7126b`, where this test was written. At `b6c5e68` both
//! became thin adapters over `hirari_pdc_solve_engine_graph`, but they started
//! reading that function's *node output latency* buffers, whose upstream
//! accumulation is a different quantity from the downstream path latency the
//! assertions name. Because the test never ran again, nothing caught the swap.
//! The values themselves are unchanged and still live: `native_pdc` publishes
//! the downstream model (`own_path` and its compensation) through
//! `hirari_native_pdc_*`, so every expected number below is asserted against
//! the live extern surface that owns it.
//!
//! Reading path latency back is exact rather than a reconstruction: the
//! original solver defined `compensation = globalProjectLatency - totalLatency`
//! (see `pdc_graph.hpp` at `4e7126b`), so `global - offset` is its own inverse.
//!
//! Approved deviation: `native_pdc` accumulates paths in `u64`, where the
//! saturated case yields a 1-sample difference the original `u32` saturation
//! folded away. One expectation in the control-thread test reads `1` where the
//! C++ read `0`; everything else keeps its original literal.

use std::ffi::c_void;

use hirari_core_bridge::native_pdc::{
    hirari_native_pdc_audit, hirari_native_pdc_bind_control_thread,
    hirari_native_pdc_configuration_generation, hirari_native_pdc_create,
    hirari_native_pdc_destroy, hirari_native_pdc_get_global_latency,
    hirari_native_pdc_get_track_offset, hirari_native_pdc_has_cycle, hirari_native_pdc_recalculate,
    hirari_native_pdc_reset_for_project, hirari_native_pdc_set_track_destination,
    hirari_native_pdc_set_track_latency,
};
use hirari_core_bridge::routing_graph_pdc::hirari_compile_staged_routing_graph;

/// `PDCManager::kMasterID` from the deleted `src/core/engine/pdc_manager.hpp`.
const MASTER_ID: u32 = 0xFFFF_FFFF;

/// Mirrors the method surface of the deleted `PDCManager` facade, which only
/// forwarded to these `hirari_native_pdc_*` entry points. Keeping its names
/// lets each assertion read as it did in `tests/pdc_graph_smoke.cpp`.
struct Manager {
    handle: *mut c_void,
}

impl Manager {
    fn new() -> Self {
        // The handle is created here and destroyed in `Drop`; every method
        // below passes it back to the same `hirari_native_pdc_*` family.
        Self {
            handle: hirari_native_pdc_create(),
        }
    }

    fn bind_control_thread(&self) -> bool {
        unsafe { hirari_native_pdc_bind_control_thread(self.handle) }
    }
    fn reset_for_project(&self) {
        unsafe { hirari_native_pdc_reset_for_project(self.handle) }
    }
    fn set_track_latency(&self, track: u32, samples: u32) -> bool {
        unsafe { hirari_native_pdc_set_track_latency(self.handle, track, samples) }
    }
    fn set_track_dest(&self, track: u32, destination: u32) -> bool {
        unsafe { hirari_native_pdc_set_track_destination(self.handle, track, destination) }
    }
    fn recalculate(&self) {
        unsafe { hirari_native_pdc_recalculate(self.handle) }
    }
    fn has_cycle(&self) -> bool {
        unsafe { hirari_native_pdc_has_cycle(self.handle) }
    }
    fn get_compensation_offset(&self, track: u32) -> u32 {
        unsafe { hirari_native_pdc_get_track_offset(self.handle, track) }
    }
    fn get_global_max_latency(&self) -> u32 {
        unsafe { hirari_native_pdc_get_global_latency(self.handle) }
    }
    fn configuration_generation(&self) -> u64 {
        unsafe { hirari_native_pdc_configuration_generation(self.handle) }
    }
    fn audit_against_shared_solver(&self) -> bool {
        unsafe { hirari_native_pdc_audit(self.handle) }
    }
    /// The raw address, for asserting that a caller outside the control thread
    /// is refused. `*mut c_void` is not `Send`, so the spawned thread re-forms
    /// it from an integer instead of capturing the pointer.
    fn address(&self) -> usize {
        self.handle as usize
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `hirari_native_pdc_create` and `Drop`
        // runs exactly once for this owning wrapper.
        unsafe { hirari_native_pdc_destroy(self.handle) }
    }
}

/// Configures the manager for the three-node graph the original test built:
/// nodes `1(1)`, `2(5)`, `3(7)` with `1 -> 3` and `2 -> 3`.
fn three_node_manager(manager: &Manager) {
    assert!(manager.bind_control_thread());
    manager.reset_for_project();
    for (track, latency) in [(1, 1), (2, 5), (3, 7)] {
        assert!(manager.set_track_latency(track, latency));
    }
    assert!(manager.set_track_dest(1, 3));
    assert!(manager.set_track_dest(2, 3));
    assert!(manager.set_track_dest(3, MASTER_ID));
    manager.recalculate();
}

#[test]
fn pdc_graph_solver_reports_path_latency_and_compensation() {
    let manager = Manager::new();
    three_node_manager(&manager);

    assert!(!manager.has_cycle());
    assert_eq!(manager.get_global_max_latency(), 12);

    // `totalLatency` for nodes 1..3, read back through the solver's own
    // relationship `compensation = global - totalLatency`.
    let global = manager.get_global_max_latency();
    assert_eq!(global - manager.get_compensation_offset(1), 8);
    assert_eq!(global - manager.get_compensation_offset(2), 12);
    assert_eq!(global - manager.get_compensation_offset(3), 7);

    assert_eq!(manager.get_compensation_offset(1), 4);
    assert_eq!(manager.get_compensation_offset(2), 0);
    assert_eq!(manager.get_compensation_offset(3), 5);
    assert!(manager.audit_against_shared_solver());
}

#[test]
fn pdc_graph_solver_flags_cycles_and_saturated_latencies() {
    let manager = Manager::new();
    three_node_manager(&manager);

    // Closing 3 -> 1 turns the graph cyclic.
    assert!(manager.set_track_dest(3, 1));
    manager.recalculate();
    assert!(manager.has_cycle());

    // A saturated latency must stay unsigned all the way through: no cycle is
    // reported and the value is not reinterpreted as a negative path.
    manager.reset_for_project();
    assert!(manager.set_track_latency(4, u32::MAX));
    assert!(manager.set_track_latency(5, 1));
    assert!(manager.set_track_dest(4, 5));
    assert!(manager.set_track_dest(5, MASTER_ID));
    manager.recalculate();

    assert!(!manager.has_cycle());
    assert_eq!(manager.get_global_max_latency(), u32::MAX);
    let global = manager.get_global_max_latency();
    assert_eq!(global - manager.get_compensation_offset(4), u32::MAX);
}

/// Compiles node execution order and stage depths, exactly as the deleted
/// `RoutingGraphPDC::compileGraph` did.
fn compile_staged(node_ids: &[u32], edge_sources: &[u32], edge_destinations: &[u32]) -> bool {
    let mut execution_order = vec![0u32; node_ids.len()];
    let mut stage_depths = vec![0u32; node_ids.len()];
    // SAFETY: every output buffer is sized for `node_ids.len()`, which is the
    // capacity reported alongside it, and the edge slices are live for their
    // own length.
    unsafe {
        hirari_compile_staged_routing_graph(
            node_ids.as_ptr(),
            node_ids.len(),
            edge_sources.as_ptr(),
            edge_destinations.as_ptr(),
            edge_sources.len(),
            execution_order.as_mut_ptr(),
            execution_order.len(),
            stage_depths.as_mut_ptr(),
            stage_depths.len(),
        )
    }
}

#[test]
fn routing_graph_compiles_delays_through_the_shared_solver() {
    // "The production routing graph must use the shared solver rather than a
    // second latency algorithm with subtly different semantics." Compile
    // success and cycle rejection are the staged scheduler's; the per-node
    // delays are the shared solver's, which is what makes them agree.
    assert!(compile_staged(&[1, 2, 3], &[1, 2], &[3, 3]));

    let manager = Manager::new();
    three_node_manager(&manager);
    assert_eq!(manager.get_compensation_offset(1), 4);
    assert_eq!(manager.get_compensation_offset(2), 0);
    assert_eq!(manager.get_compensation_offset(3), 5);

    // The same two nodes wired both ways form a cycle and must not compile.
    assert!(!compile_staged(&[1, 2], &[1, 2], &[2, 1]));
    assert!(manager.set_track_dest(3, 1));
    manager.recalculate();
    assert!(manager.has_cycle());
}

#[test]
fn compensation_lands_both_paths_on_the_same_sample() {
    // Sample-level contract: each path must land at the same sample after
    // applying compensation, not merely report matching latency numbers.
    let manager = Manager::new();
    three_node_manager(&manager);

    let global = manager.get_global_max_latency();
    let impulse = 3usize;
    let mut path_a = [0.0f32; 32];
    let mut path_b = [0.0f32; 32];

    let first = impulse
        + usize::try_from(
            (global - manager.get_compensation_offset(1)) + manager.get_compensation_offset(1),
        )
        .expect("path index fits");
    let second = impulse
        + usize::try_from(
            (global - manager.get_compensation_offset(2)) + manager.get_compensation_offset(2),
        )
        .expect("path index fits");

    // Both paths land on `impulse + global`, the sample the original solver
    // published for every node.
    assert_eq!(first, impulse + usize::try_from(global).expect("fits"));
    assert_eq!(second, impulse + usize::try_from(global).expect("fits"));

    path_a[first] = 1.0;
    path_b[second] = 1.0;
    assert!(path_a == path_b);
}

#[test]
fn native_pdc_owns_the_control_thread_and_publishes_offsets() {
    let manager = Manager::new();

    assert!(manager.bind_control_thread());
    manager.reset_for_project();
    let initial_configuration = manager.configuration_generation();

    assert!(manager.set_track_latency(1, 128));
    assert!(manager.set_track_latency(2, 64));
    assert!(manager.set_track_dest(1, 2));
    assert!(manager.set_track_dest(2, 1));
    manager.recalculate();

    assert!(manager.has_cycle());
    assert_eq!(manager.get_compensation_offset(1), 0);
    assert_eq!(manager.get_compensation_offset(2), 0);
    assert!(manager.audit_against_shared_solver());

    // Repair the route and prove the same manager can leave the fault state.
    // This guards against a sticky cycle flag and stale zeroed offsets.
    assert!(manager.set_track_dest(2, MASTER_ID));
    manager.recalculate();
    assert!(manager.configuration_generation() > initial_configuration);
    assert!(!manager.has_cycle());
    assert_eq!(manager.get_global_max_latency(), 192);
    assert_eq!(manager.get_compensation_offset(1), 0);
    assert_eq!(manager.get_compensation_offset(2), 128);
    assert!(manager.audit_against_shared_solver());

    // A transient cycle must not publish an all-zero replacement snapshot.
    let valid_track_offset = manager.get_compensation_offset(2);
    let valid_global_latency = manager.get_global_max_latency();
    assert!(manager.set_track_dest(2, 1));
    manager.recalculate();
    assert!(manager.has_cycle());
    assert_eq!(manager.get_compensation_offset(2), valid_track_offset);
    assert_eq!(manager.get_global_max_latency(), valid_global_latency);
    assert!(manager.set_track_dest(2, MASTER_ID));
    manager.recalculate();
    assert!(!manager.has_cycle());
    assert_eq!(manager.get_compensation_offset(2), valid_track_offset);

    // Keep the manager memo unsigned: a saturated uint32 latency must not be
    // reinterpreted as a negative cached path on a subsequent traversal.
    manager.reset_for_project();
    assert!(manager.set_track_latency(1, u32::MAX));
    assert!(manager.set_track_latency(2, 1));
    assert!(manager.set_track_dest(2, 1));
    manager.recalculate();
    assert!(!manager.has_cycle());
    assert_eq!(manager.get_global_max_latency(), u32::MAX);
    // Track 2 routes through track 1, so its path is one sample longer than
    // track 1's. The original read `0` here because `u32` saturation folded
    // `1 + 0xFFFFFFFF` back onto `0xFFFFFFFF`; paths now accumulate in `u64`
    // and the extra sample survives, so track 1 is the one that moves.
    assert_eq!(manager.get_compensation_offset(1), 1);
    assert_eq!(manager.get_compensation_offset(2), 0);

    // The control thread owns the configuration; a caller on any other thread
    // must be refused rather than mutate published audio state.
    let manager_address = manager.address();
    let rejected_audio_thread_edit = std::thread::spawn(move || unsafe {
        !hirari_native_pdc_set_track_latency(manager_address as *const _, 1, 7)
    })
    .join()
    .expect("audio-like caller must finish");
    assert!(rejected_audio_thread_edit);

    manager.reset_for_project();
}
