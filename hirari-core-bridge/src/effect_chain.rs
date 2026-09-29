//! Realtime traversal and parallel wet/dry mixing for Track and Bus effects.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

#[repr(C)]
pub struct EffectChainNodeView {
    pub processor: *mut c_void,
    pub bypassed: u8,
    pub parallel: u8,
    pub latency_samples: u32,
}

struct RuntimeNode {
    processor: usize,
    bypassed: bool,
    parallel: bool,
}

struct RuntimeSnapshot {
    nodes: Box<[RuntimeNode]>,
}

struct RuntimeEffectChain {
    current: AtomicPtr<RuntimeSnapshot>,
    retired: Mutex<Vec<Box<RuntimeSnapshot>>>,
    total_latency_samples: AtomicU32,
    audio_readers: AtomicU32,
    audio_mutation: AtomicBool,
}

pub type EffectChainMetricValue = unsafe extern "C" fn(processor: *mut c_void, metric: u32) -> u64;

struct RuntimeProcessContext {
    snapshot: *const RuntimeSnapshot,
    user_data: *mut c_void,
    process_node: EffectNodeProcessWithHandle,
}

pub type EffectNodeProcessWithHandle = unsafe extern "C" fn(
    user_data: *mut c_void,
    index: u32,
    processor: *mut c_void,
    parallel: bool,
    mix: *mut f32,
) -> bool;

#[no_mangle]
pub extern "C" fn hirari_effect_chain_runtime_create() -> *mut c_void {
    let initial = Box::into_raw(Box::new(RuntimeSnapshot {
        nodes: Box::new([]),
    }));
    Box::into_raw(Box::new(RuntimeEffectChain {
        current: AtomicPtr::new(initial),
        retired: Mutex::new(Vec::new()),
        total_latency_samples: AtomicU32::new(0),
        audio_readers: AtomicU32::new(0),
        audio_mutation: AtomicBool::new(false),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_enter_audio(state: *const c_void) -> bool {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return false;
    };
    if runtime.audio_mutation.load(Ordering::SeqCst) {
        return false;
    }
    runtime.audio_readers.fetch_add(1, Ordering::SeqCst);
    if runtime.audio_mutation.load(Ordering::SeqCst) {
        runtime.audio_readers.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_leave_audio(state: *const c_void) {
    if let Some(runtime) = unsafe { state.cast::<RuntimeEffectChain>().as_ref() } {
        runtime.audio_readers.fetch_sub(1, Ordering::SeqCst);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_begin_mutation(state: *const c_void) {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return;
    };
    runtime.audio_mutation.store(true, Ordering::SeqCst);
    while runtime.audio_readers.load(Ordering::SeqCst) != 0 {
        std::thread::yield_now();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_end_mutation(state: *const c_void) {
    if let Some(runtime) = unsafe { state.cast::<RuntimeEffectChain>().as_ref() } {
        runtime.audio_mutation.store(false, Ordering::SeqCst);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_audio_reader_count(
    state: *const c_void,
) -> u32 {
    unsafe { state.cast::<RuntimeEffectChain>().as_ref() }
        .map_or(0, |runtime| runtime.audio_readers.load(Ordering::SeqCst))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let runtime = unsafe { Box::from_raw(state.cast::<RuntimeEffectChain>()) };
    let current = runtime.current.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !current.is_null() {
        unsafe { drop(Box::from_raw(current)) };
    }
    // Retired snapshots contain raw processor addresses only. Drop their
    // metadata without touching processor ownership, which stays in C++.
    drop(runtime);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_publish(
    state: *mut c_void,
    nodes: *const EffectChainNodeView,
    count: usize,
) -> bool {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return false;
    };
    if count > 0 && nodes.is_null() {
        return false;
    }
    let views = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(nodes, count) }
    };
    let mut next_nodes = Vec::with_capacity(count);
    let mut total_latency = 0_u32;
    for view in views {
        if view.processor.is_null() {
            return false;
        }
        next_nodes.push(RuntimeNode {
            processor: view.processor as usize,
            bypassed: view.bypassed != 0,
            parallel: view.parallel != 0,
        });
        if view.bypassed == 0 {
            total_latency = total_latency.wrapping_add(view.latency_samples);
        }
    }
    let next = Box::into_raw(Box::new(RuntimeSnapshot {
        nodes: next_nodes.into_boxed_slice(),
    }));
    let previous = runtime.current.swap(next, Ordering::SeqCst);
    runtime
        .total_latency_samples
        .store(total_latency, Ordering::Relaxed);
    if !previous.is_null() {
        runtime
            .retired
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(unsafe { Box::from_raw(previous) });
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_total_latency_samples(
    state: *const c_void,
) -> u32 {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return 0;
    };
    runtime.total_latency_samples.load(Ordering::Relaxed)
}

/// Fold processor diagnostics and effect tails over the current immutable
/// chain snapshot. Metric 0 drains watchdog trips, 1 reads sanitized sample
/// counts, and 2 calculates the serial plus longest-parallel effect tail.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_metric(
    state: *const c_void,
    metric: u32,
    read_value: Option<EffectChainMetricValue>,
) -> u64 {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return 0;
    };
    let Some(read_value) = read_value else {
        return 0;
    };
    let snapshot = runtime.current.load(Ordering::SeqCst);
    if snapshot.is_null() {
        return 0;
    }
    let mut total = 0_u64;
    let mut serial_tail = 0_u64;
    let mut parallel_tail = 0_u32;
    for node in unsafe { &(*snapshot).nodes }.iter() {
        match metric {
            0 | 1 => {
                total = total
                    .wrapping_add(unsafe { read_value(node.processor as *mut c_void, metric) });
            }
            2 if !node.bypassed => {
                let tail = unsafe { read_value(node.processor as *mut c_void, metric) } as u32;
                if node.parallel {
                    parallel_tail = parallel_tail.max(tail);
                } else {
                    serial_tail = serial_tail
                        .saturating_add(u64::from(tail))
                        .min(u64::from(u32::MAX));
                }
            }
            2 => {}
            _ => return 0,
        }
    }
    if metric == 2 {
        serial_tail
            .saturating_add(u64::from(parallel_tail))
            .min(u64::from(u32::MAX))
    } else {
        total
    }
}

/// Reclaims old pointer tables after the C++ processor owner confirms its
/// callback reader count reached zero.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_reclaim(state: *const c_void) {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return;
    };
    runtime
        .retired
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_processor_at(
    state: *const c_void,
    index: u32,
) -> *mut c_void {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    let snapshot = runtime.current.load(Ordering::SeqCst);
    if snapshot.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { (&(*snapshot).nodes).get(index as usize) }
        .map_or(std::ptr::null_mut(), |node| node.processor as *mut c_void)
}

unsafe extern "C" fn runtime_node_info(
    context: *mut c_void,
    index: u32,
    bypassed: *mut bool,
    parallel: *mut bool,
) -> bool {
    if context.is_null() || bypassed.is_null() || parallel.is_null() {
        return false;
    }
    let context = unsafe { &*context.cast::<RuntimeProcessContext>() };
    let Some(node) = (unsafe { &*context.snapshot }).nodes.get(index as usize) else {
        return false;
    };
    unsafe {
        bypassed.write(node.bypassed);
        parallel.write(node.parallel);
    }
    true
}

unsafe extern "C" fn runtime_process_node(
    context: *mut c_void,
    index: u32,
    parallel: bool,
    mix: *mut f32,
) -> bool {
    if context.is_null() || mix.is_null() {
        return false;
    }
    let context = unsafe { &*context.cast::<RuntimeProcessContext>() };
    let Some(node) = (unsafe { &*context.snapshot }).nodes.get(index as usize) else {
        return false;
    };
    unsafe {
        (context.process_node)(
            context.user_data,
            index,
            node.processor as *mut c_void,
            parallel,
            mix,
        )
    }
}

/// Runs a published Rust snapshot. The host's realtime reader guard must stay
/// alive for this whole call and must gate snapshot reclamation.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_runtime_process_block(
    state: *const c_void,
    user_data: *mut c_void,
    channels: *const *mut f32,
    channel_count: u32,
    parallel_channels: *const *mut f32,
    parallel_channel_count: u32,
    parallel_capacity: u32,
    frames: u32,
    process_node: Option<EffectNodeProcessWithHandle>,
) {
    let Some(runtime) = (unsafe { state.cast::<RuntimeEffectChain>().as_ref() }) else {
        return;
    };
    let (Some(process_node), snapshot) = (process_node, runtime.current.load(Ordering::SeqCst))
    else {
        return;
    };
    if snapshot.is_null() {
        return;
    }
    let mut context = RuntimeProcessContext {
        snapshot,
        user_data,
        process_node,
    };
    unsafe {
        hirari_effect_chain_process_block(
            (&mut context as *mut RuntimeProcessContext).cast(),
            (&(*snapshot).nodes).len() as u32,
            channels,
            channel_count,
            parallel_channels,
            parallel_channel_count,
            parallel_capacity,
            frames,
            Some(runtime_node_info),
            Some(runtime_process_node),
        );
    }
}

pub type EffectNodeInfo = unsafe extern "C" fn(
    user_data: *mut c_void,
    index: u32,
    bypassed: *mut bool,
    parallel: *mut bool,
) -> bool;
pub type EffectNodeProcess =
    unsafe extern "C" fn(user_data: *mut c_void, index: u32, parallel: bool, mix: *mut f32) -> bool;

unsafe fn clear_channels(channels: *const *mut f32, channel_count: u32, frames: usize) {
    if channels.is_null() {
        return;
    }
    for channel in 0..channel_count as usize {
        let samples = unsafe { *channels.add(channel) };
        if samples.is_null() {
            continue;
        }
        for frame in 0..frames {
            unsafe { samples.add(frame).write(0.0) };
        }
    }
}

unsafe fn sanitize_channels(channels: *const *mut f32, channel_count: u32, frames: usize) {
    if channels.is_null() {
        return;
    }
    for channel in 0..channel_count as usize {
        let samples = unsafe { *channels.add(channel) };
        if samples.is_null() {
            continue;
        }
        for frame in 0..frames {
            let sample = unsafe { samples.add(frame) };
            if !unsafe { *sample }.is_finite() {
                unsafe { sample.write(0.0) };
            }
        }
    }
}

#[cfg(test)]
mod metric_tests {
    use super::*;

    struct MetricValues {
        watchdog_trips: u64,
        non_finite_samples: u64,
        tail_samples: u64,
    }

    unsafe extern "C" fn read_metric(processor: *mut c_void, metric: u32) -> u64 {
        let values = unsafe { &*processor.cast::<MetricValues>() };
        match metric {
            0 => values.watchdog_trips,
            1 => values.non_finite_samples,
            2 => values.tail_samples,
            _ => 0,
        }
    }

    #[test]
    fn aggregation_preserves_latency_tail_and_diagnostic_rules() {
        let values = [
            MetricValues {
                watchdog_trips: 1,
                non_finite_samples: 10,
                tail_samples: 100,
            },
            MetricValues {
                watchdog_trips: 0,
                non_finite_samples: 20,
                tail_samples: 300,
            },
            MetricValues {
                watchdog_trips: 1,
                non_finite_samples: 30,
                tail_samples: 1_000,
            },
            MetricValues {
                watchdog_trips: 1,
                non_finite_samples: 40,
                tail_samples: 250,
            },
        ];
        let views = [
            EffectChainNodeView {
                processor: (&values[0] as *const MetricValues).cast_mut().cast(),
                bypassed: 0,
                parallel: 0,
                latency_samples: u32::MAX - 5,
            },
            EffectChainNodeView {
                processor: (&values[1] as *const MetricValues).cast_mut().cast(),
                bypassed: 0,
                parallel: 1,
                latency_samples: 10,
            },
            EffectChainNodeView {
                processor: (&values[2] as *const MetricValues).cast_mut().cast(),
                bypassed: 1,
                parallel: 1,
                latency_samples: 5,
            },
            EffectChainNodeView {
                processor: (&values[3] as *const MetricValues).cast_mut().cast(),
                bypassed: 0,
                parallel: 1,
                latency_samples: 6,
            },
        ];
        let state = hirari_effect_chain_runtime_create();
        assert!(!state.is_null());
        assert!(unsafe { hirari_effect_chain_runtime_publish(state, views.as_ptr(), views.len()) });

        assert_eq!(
            unsafe { hirari_effect_chain_runtime_total_latency_samples(state) },
            10
        );
        assert_eq!(
            unsafe { hirari_effect_chain_runtime_metric(state, 0, Some(read_metric)) },
            3
        );
        assert_eq!(
            unsafe { hirari_effect_chain_runtime_metric(state, 1, Some(read_metric)) },
            100
        );
        // 100 serial frames plus the longest parallel branch (300); the
        // bypassed 1,000-frame branch and shorter 250-frame branch do not add.
        assert_eq!(
            unsafe { hirari_effect_chain_runtime_metric(state, 2, Some(read_metric)) },
            400
        );
        unsafe { hirari_effect_chain_runtime_destroy(state) };
    }
}

/// Runs one immutable C++ processor-list generation without allocating or
/// taking locks. C++ callbacks retain ownership and invoke plugin interfaces.
///
/// # Safety
/// Channel tables and their disjoint sample buffers must remain valid for the
/// duration of this call. Callbacks must be valid C ABI functions and must not
/// unwind across the ABI boundary.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_process_block(
    user_data: *mut c_void,
    node_count: u32,
    channels: *const *mut f32,
    channel_count: u32,
    parallel_channels: *const *mut f32,
    parallel_channel_count: u32,
    parallel_capacity: u32,
    frames: u32,
    node_info: Option<EffectNodeInfo>,
    process_node: Option<EffectNodeProcess>,
) {
    if channel_count > 0 && channels.is_null() {
        return;
    }
    let (Some(node_info), Some(process_node)) = (node_info, process_node) else {
        return;
    };
    let frames = frames as usize;
    for index in 0..node_count {
        let mut bypassed = false;
        let mut parallel = false;
        if !unsafe { node_info(user_data, index, &mut bypassed, &mut parallel) } || bypassed {
            continue;
        }
        let use_parallel = parallel
            && channel_count <= 2
            && channel_count <= parallel_channel_count
            && frames <= parallel_capacity as usize
            && !parallel_channels.is_null();
        if use_parallel {
            for channel in 0..channel_count as usize {
                let source = unsafe { *channels.add(channel) };
                let destination = unsafe { *parallel_channels.add(channel) };
                if source.is_null() || destination.is_null() {
                    continue;
                }
                unsafe { std::ptr::copy(source, destination, frames) };
            }
        }

        let mut mix = 1.0_f32;
        if !unsafe { process_node(user_data, index, use_parallel, &mut mix) } {
            unsafe { clear_channels(channels, channel_count, frames) };
            continue;
        }

        if use_parallel {
            mix = mix.clamp(0.0, 1.0);
            for channel in 0..channel_count as usize {
                let dry = unsafe { *channels.add(channel) };
                let wet = unsafe { *parallel_channels.add(channel) };
                if dry.is_null() || wet.is_null() {
                    continue;
                }
                for frame in 0..frames {
                    let dry_sample = unsafe { dry.add(frame) };
                    let wet_sample = unsafe { wet.add(frame) };
                    unsafe {
                        dry_sample.write(*dry_sample * (1.0 - mix) + *wet_sample * mix);
                    }
                }
            }
        }
        unsafe { sanitize_channels(channels, channel_count, frames) };
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod differential_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_effect_chain_process_reference(
            user_data: *mut c_void,
            node_count: u32,
            channels: *const *mut f32,
            channel_count: u32,
            parallel_channels: *const *mut f32,
            parallel_channel_count: u32,
            parallel_capacity: u32,
            frames: u32,
            node_info: Option<EffectNodeInfo>,
            process_node: Option<EffectNodeProcess>,
        );
    }

    #[derive(Clone, Copy)]
    struct TestNode {
        bypassed: bool,
        parallel: bool,
        gain: f32,
        mix: f32,
        fail: bool,
    }
    struct TestContext {
        nodes: [TestNode; 6],
        dry: [*mut f32; 2],
        wet: [*mut f32; 2],
        frames: usize,
    }

    unsafe extern "C" fn test_node_info(
        user_data: *mut c_void,
        index: u32,
        bypassed: *mut bool,
        parallel: *mut bool,
    ) -> bool {
        let context = unsafe { &*user_data.cast::<TestContext>() };
        let Some(node) = context.nodes.get(index as usize) else {
            return false;
        };
        unsafe {
            bypassed.write(node.bypassed);
            parallel.write(node.parallel);
        }
        true
    }

    unsafe extern "C" fn test_process_node(
        user_data: *mut c_void,
        index: u32,
        parallel: bool,
        mix: *mut f32,
    ) -> bool {
        let context = unsafe { &*user_data.cast::<TestContext>() };
        let Some(node) = context.nodes.get(index as usize) else {
            return false;
        };
        if node.fail {
            return false;
        }
        let channels = if parallel { &context.wet } else { &context.dry };
        for channel in channels {
            for frame in 0..context.frames {
                unsafe {
                    let sample = channel.add(frame);
                    sample.write(*sample * node.gain);
                }
            }
        }
        unsafe { mix.write(node.mix) };
        true
    }

    #[test]
    fn wet_dry_chain_matches_the_previous_cpp_traversal() {
        const FRAMES: usize = 73;
        let nodes = [
            TestNode {
                bypassed: false,
                parallel: true,
                gain: 0.5,
                mix: 0.25,
                fail: false,
            },
            TestNode {
                bypassed: true,
                parallel: false,
                gain: 4.0,
                mix: 1.0,
                fail: false,
            },
            TestNode {
                bypassed: false,
                parallel: false,
                gain: 1.25,
                mix: 1.0,
                fail: false,
            },
            TestNode {
                bypassed: false,
                parallel: true,
                gain: 2.0,
                mix: 0.7,
                fail: false,
            },
            TestNode {
                bypassed: false,
                parallel: false,
                gain: 1.0,
                mix: 1.0,
                fail: true,
            },
            TestNode {
                bypassed: false,
                parallel: false,
                gain: 0.8,
                mix: 1.0,
                fail: false,
            },
        ];
        let mut rust_l = (0..FRAMES)
            .map(|i| (i as f32 * 0.031).sin())
            .collect::<Vec<_>>();
        let mut rust_r = (0..FRAMES)
            .map(|i| (i as f32 * 0.017).cos())
            .collect::<Vec<_>>();
        let mut cpp_l = rust_l.clone();
        let mut cpp_r = rust_r.clone();
        let mut rust_wet_l = [0.0; FRAMES];
        let mut rust_wet_r = [0.0; FRAMES];
        let mut cpp_wet_l = [0.0; FRAMES];
        let mut cpp_wet_r = [0.0; FRAMES];
        rust_l[7] = f32::NAN;
        cpp_l[7] = f32::NAN;
        rust_r[11] = f32::INFINITY;
        cpp_r[11] = f32::INFINITY;
        let rust_channels = [rust_l.as_mut_ptr(), rust_r.as_mut_ptr()];
        let rust_parallel = [rust_wet_l.as_mut_ptr(), rust_wet_r.as_mut_ptr()];
        let cpp_channels = [cpp_l.as_mut_ptr(), cpp_r.as_mut_ptr()];
        let cpp_parallel = [cpp_wet_l.as_mut_ptr(), cpp_wet_r.as_mut_ptr()];
        let mut rust_context = TestContext {
            nodes,
            dry: rust_channels,
            wet: rust_parallel,
            frames: FRAMES,
        };
        let mut cpp_context = TestContext {
            nodes,
            dry: cpp_channels,
            wet: cpp_parallel,
            frames: FRAMES,
        };
        unsafe {
            hirari_effect_chain_process_block(
                (&mut rust_context as *mut TestContext).cast(),
                nodes.len() as u32,
                rust_channels.as_ptr(),
                2,
                rust_parallel.as_ptr(),
                2,
                FRAMES as u32,
                FRAMES as u32,
                Some(test_node_info),
                Some(test_process_node),
            );
            hirari_effect_chain_process_reference(
                (&mut cpp_context as *mut TestContext).cast(),
                nodes.len() as u32,
                cpp_channels.as_ptr(),
                2,
                cpp_parallel.as_ptr(),
                2,
                FRAMES as u32,
                FRAMES as u32,
                Some(test_node_info),
                Some(test_process_node),
            );
        }
        for (actual, expected) in [
            rust_l.as_slice().iter().zip(&cpp_l),
            rust_r.as_slice().iter().zip(&cpp_r),
            rust_wet_l.iter().zip(&cpp_wet_l),
            rust_wet_r.iter().zip(&cpp_wet_r),
        ]
        .into_iter()
        .flatten()
        {
            let tolerance = 1.0e-6 + 2.0e-7 * actual.abs().max(expected.abs());
            assert!((actual - expected).abs() <= tolerance);
        }
        assert!(rust_l.iter().all(|sample| sample.is_finite()));
        assert!(rust_r.iter().all(|sample| sample.is_finite()));
    }
}
