//! Ownership, ordering and control-side mutation for Track/Bus effect chains.
//!
//! The C++ side keeps `DSP::IProcessor` objects but no longer stores the chain:
//! each entry is a boxed `std::shared_ptr<IProcessor>` handed over here, and a
//! Rust-owned ordered list decides what the realtime snapshot traverses. The
//! snapshot/reader machinery lives in `effect_chain.rs`; this module drives it
//! and owns the processor lifetimes across generations.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::effect_chain;

/// C++ adapters for one processor. Every entry must be a valid function
/// pointer for the lifetime of the chain.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EffectNodeOps {
    pub prepare: unsafe extern "C" fn(*mut c_void, f64, u32),
    pub reset: unsafe extern "C" fn(*mut c_void),
    pub set_parameter: unsafe extern "C" fn(*mut c_void, u32, f32),
    pub set_parameter_at_sample: unsafe extern "C" fn(*mut c_void, u32, f32, u32),
    pub latency_samples: unsafe extern "C" fn(*mut c_void) -> u32,
    pub state_size: unsafe extern "C" fn(*mut c_void) -> usize,
    pub state_copy: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize,
    pub state_restore: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool,
    pub gui_state_size: unsafe extern "C" fn(*mut c_void) -> usize,
    pub gui_state_copy: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize,
    pub gui_state_restore: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool,
    pub num_parameters: unsafe extern "C" fn(*mut c_void) -> u32,
    pub parameter: unsafe extern "C" fn(*mut c_void, u32) -> f32,
    pub clone_owner: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub destroy_owner: unsafe extern "C" fn(*mut c_void),
    pub read_metric: unsafe extern "C" fn(*mut c_void, u32) -> u64,
}

/// Mirrors `DSP::TimedParameterEvent` on the C++ side.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimedParameterEvent {
    pub processor_index: u32,
    pub parameter_id: u32,
    pub sample_offset: u32,
    pub normalized_value: f32,
}

struct HostNode {
    owner: *mut c_void,
    processor: *mut c_void,
    bypassed: bool,
    parallel: bool,
}

struct HostInner {
    nodes: Vec<HostNode>,
    retired: Vec<*mut c_void>,
    sample_rate: f64,
    block_size: u32,
}

// The raw pointers are only dereferenced through C++ adapters on the control
// thread; the realtime side reads the immutable Rust snapshot instead.
unsafe impl Send for HostInner {}

pub struct EffectChainHost {
    ops: EffectNodeOps,
    runtime: *mut c_void,
    inner: Mutex<HostInner>,
}

unsafe fn host<'a>(state: *mut c_void) -> Option<&'a EffectChainHost> {
    unsafe { state.cast::<EffectChainHost>().as_ref() }
}

unsafe fn inner<'a>(host: &'a EffectChainHost) -> std::sync::MutexGuard<'a, HostInner> {
    host.inner.lock().unwrap_or_else(|error| error.into_inner())
}

unsafe fn publish(host: &EffectChainHost, inner: &HostInner) {
    let mut views = Vec::with_capacity(inner.nodes.len());
    for node in &inner.nodes {
        views.push(effect_chain::EffectChainNodeView {
            processor: node.processor,
            bypassed: node.bypassed as u8,
            parallel: node.parallel as u8,
            latency_samples: unsafe { (host.ops.latency_samples)(node.processor) },
        });
    }
    unsafe {
        effect_chain::hirari_effect_chain_runtime_publish(
            host.runtime,
            views.as_ptr(),
            views.len(),
        )
    };
}

unsafe fn begin_mutation(host: &EffectChainHost) {
    unsafe { effect_chain::hirari_effect_chain_runtime_begin_mutation(host.runtime) };
}

unsafe fn end_mutation(host: &EffectChainHost) {
    unsafe { effect_chain::hirari_effect_chain_runtime_end_mutation(host.runtime) };
}

unsafe fn reader_count(host: &EffectChainHost) -> u32 {
    unsafe { effect_chain::hirari_effect_chain_runtime_audio_reader_count(host.runtime) }
}

unsafe fn free_owners(host: &EffectChainHost, owners: &mut Vec<*mut c_void>) {
    for owner in owners.drain(..) {
        if !owner.is_null() {
            unsafe { (host.ops.destroy_owner)(owner) };
        }
    }
}

unsafe fn reclaim_locked(host: &EffectChainHost, inner: &mut HostInner) {
    if unsafe { reader_count(host) } != 0 {
        return;
    }
    unsafe {
        effect_chain::hirari_effect_chain_runtime_reclaim(host.runtime);
        free_owners(host, &mut inner.retired);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_create(ops: *const EffectNodeOps) -> *mut c_void {
    let Some(ops) = (unsafe { ops.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let runtime = effect_chain::hirari_effect_chain_runtime_create();
    if runtime.is_null() {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(EffectChainHost {
        ops: *ops,
        runtime,
        inner: Mutex::new(HostInner {
            nodes: Vec::new(),
            retired: Vec::new(),
            sample_rate: 0.0,
            block_size: 0,
        }),
    }))
    .cast()
}

/// Waits for audio readers, then releases every processor the chain owns. A
/// callback that cannot quiesce within the bounded wait leaks the whole chain
/// rather than freeing memory a realtime thread may still traverse.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let host = unsafe { Box::from_raw(state.cast::<EffectChainHost>()) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { reader_count(&host) } != 0 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    if unsafe { reader_count(&host) } != 0 {
        Box::leak(host);
        return;
    }
    let runtime = host.runtime;
    {
        let mut guard = unsafe { inner(&host) };
        free_owners(&host, &mut guard.retired);
        let mut nodes = std::mem::take(&mut guard.nodes);
        for node in &mut nodes {
            if !node.owner.is_null() {
                unsafe { (host.ops.destroy_owner)(node.owner) };
            }
        }
    }
    unsafe { effect_chain::hirari_effect_chain_runtime_destroy(runtime) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_add(
    state: *mut c_void,
    owner: *mut c_void,
    processor: *mut c_void,
    parallel: bool,
) -> bool {
    unsafe { hirari_effect_chain_insert(state, u32::MAX, owner, processor, false, parallel) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_insert(
    state: *mut c_void,
    index: u32,
    owner: *mut c_void,
    processor: *mut c_void,
    bypassed: bool,
    parallel: bool,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    if owner.is_null() || processor.is_null() {
        return false;
    }
    let mut guard = unsafe { inner(host) };
    let position = (index as usize).min(guard.nodes.len());
    if index != u32::MAX && index as usize > guard.nodes.len() {
        return false;
    }
    if guard.sample_rate > 0.0 && guard.block_size > 0 {
        unsafe { (host.ops.prepare)(processor, guard.sample_rate, guard.block_size) };
    }
    guard.nodes.insert(
        position,
        HostNode {
            owner,
            processor,
            bypassed,
            parallel,
        },
    );
    unsafe { publish(host, &guard) };
    true
}

/// Removes one entry and returns a freshly boxed `shared_ptr` copy for the
/// caller while the chain keeps its own owner alive until readers drain.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_remove(
    state: *mut c_void,
    index: u32,
    bypassed: *mut bool,
    parallel: *mut bool,
) -> *mut c_void {
    let Some(host) = (unsafe { host(state) }) else {
        return std::ptr::null_mut();
    };
    let mut guard = unsafe { inner(host) };
    if index as usize >= guard.nodes.len() {
        return std::ptr::null_mut();
    }
    let node = guard.nodes.remove(index as usize);
    if !bypassed.is_null() {
        unsafe { bypassed.write(node.bypassed) };
    }
    if !parallel.is_null() {
        unsafe { parallel.write(node.parallel) };
    }
    let clone = unsafe { (host.ops.clone_owner)(node.owner) };
    guard.retired.push(node.owner);
    unsafe { publish(host, &guard) };
    unsafe { reclaim_locked(host, &mut guard) };
    clone
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_move(
    state: *mut c_void,
    from: u32,
    to: u32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let mut guard = unsafe { inner(host) };
    let count = guard.nodes.len();
    if from as usize >= count || to as usize >= count || from == to {
        return false;
    }
    let node = guard.nodes.remove(from as usize);
    guard.nodes.insert(to as usize, node);
    unsafe { publish(host, &guard) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_clear(state: *mut c_void) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let mut guard = unsafe { inner(host) };
    let mut cleared = std::mem::take(&mut guard.nodes);
    guard.retired.extend(cleared.drain(..).map(|node| node.owner));
    unsafe { publish(host, &guard) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_count(state: *const c_void) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    unsafe { inner(host) }.nodes.len() as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_processor(
    state: *const c_void,
    index: u32,
) -> *mut c_void {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return std::ptr::null_mut();
    };
    unsafe { inner(host) }
        .nodes
        .get(index as usize)
        .map_or(std::ptr::null_mut(), |node| node.processor)
}

/// Boxed `shared_ptr` copy for a processor the caller wants to hold. The
/// caller owns the returned box and destroys it on its own side.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_owner_clone(
    state: *const c_void,
    index: u32,
) -> *mut c_void {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return std::ptr::null_mut();
    };
    let guard = unsafe { inner(host) };
    guard.nodes.get(index as usize).map_or(std::ptr::null_mut(), |node| {
        unsafe { (host.ops.clone_owner)(node.owner) }
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_set_bypass(
    state: *mut c_void,
    index: u32,
    bypassed: bool,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let mut guard = unsafe { inner(host) };
    let Some(node) = guard.nodes.get_mut(index as usize) else {
        return false;
    };
    node.bypassed = bypassed;
    unsafe { publish(host, &guard) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_bypass(state: *const c_void, index: u32) -> bool {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return false;
    };
    unsafe { inner(host) }
        .nodes
        .get(index as usize)
        .is_some_and(|node| node.bypassed)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
    block_size: u32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    if !sample_rate.is_finite() || sample_rate <= 0.0 || block_size == 0 {
        return false;
    }
    let mut guard = unsafe { inner(host) };
    unsafe { begin_mutation(host) };
    guard.sample_rate = sample_rate;
    guard.block_size = block_size;
    for node in &guard.nodes {
        unsafe { (host.ops.prepare)(node.processor, sample_rate, block_size) };
    }
    unsafe {
        end_mutation(host);
        publish(host, &guard);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_reset(state: *mut c_void) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let guard = unsafe { inner(host) };
    unsafe { begin_mutation(host) };
    for node in &guard.nodes {
        unsafe { (host.ops.reset)(node.processor) };
    }
    unsafe { end_mutation(host) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_set_parameter(
    state: *mut c_void,
    index: u32,
    parameter_id: u32,
    value: f32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    if !value.is_finite() {
        return false;
    }
    let guard = unsafe { inner(host) };
    let Some(node) = guard.nodes.get(index as usize) else {
        return false;
    };
    unsafe { begin_mutation(host) };
    unsafe { (host.ops.set_parameter)(node.processor, parameter_id, value.clamp(0.0, 1.0)) };
    unsafe { end_mutation(host) };
    true
}

/// Applies sample-accurate plugin automation inside one reader generation.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_parameter_automation(
    state: *mut c_void,
    events: *const TimedParameterEvent,
    event_count: usize,
) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    if events.is_null() || event_count == 0 {
        return;
    }
    if !unsafe { effect_chain::hirari_effect_chain_runtime_enter_audio(host.runtime) } {
        return;
    }
    let guard = unsafe { inner(host) };
    let events = unsafe { std::slice::from_raw_parts(events, event_count) };
    for event in events {
        if !event.normalized_value.is_finite() {
            continue;
        }
        let Some(node) = guard.nodes.get(event.processor_index as usize) else {
            continue;
        };
        unsafe {
            (host.ops.set_parameter_at_sample)(
                node.processor,
                event.parameter_id,
                event.normalized_value,
                event.sample_offset,
            );
        }
    }
    drop(guard);
    unsafe { effect_chain::hirari_effect_chain_runtime_leave_audio(host.runtime) };
}

unsafe fn read_state(
    host: &EffectChainHost,
    index: u32,
    gui: bool,
    allow_marker: bool,
) -> Option<Vec<u8>> {
    let guard = unsafe { inner(host) };
    let node = guard.nodes.get(index as usize)?;
    let size = unsafe {
        if gui {
            (host.ops.gui_state_size)(node.processor)
        } else {
            (host.ops.state_size)(node.processor)
        }
    };
    let mut state = vec![0_u8; size];
    if size > 0 {
        let written = unsafe {
            if gui {
                (host.ops.gui_state_copy)(node.processor, state.as_mut_ptr(), size)
            } else {
                (host.ops.state_copy)(node.processor, state.as_mut_ptr(), size)
            }
        };
        state.truncate(written.min(size));
    }
    if state.is_empty() && allow_marker && unsafe { (host.ops.num_parameters)(node.processor) } > 0 {
        // Keep project snapshots structurally complete for legacy processors
        // without a serializer: a deterministic non-empty marker.
        let value = unsafe { (host.ops.parameter)(node.processor, 0) };
        state.extend_from_slice(&value.to_le_bytes());
    }
    Some(state)
}

unsafe fn restore_state(
    host: &EffectChainHost,
    index: u32,
    data: *const u8,
    length: usize,
    gui: bool,
) -> bool {
    if length != 0 && data.is_null() {
        return false;
    }
    let guard = unsafe { inner(host) };
    let Some(node) = guard.nodes.get(index as usize) else {
        return false;
    };
    unsafe { begin_mutation(host) };
    let restored = unsafe {
        if gui {
            (host.ops.gui_state_restore)(node.processor, data, length)
        } else {
            (host.ops.state_restore)(node.processor, data, length)
        }
    };
    unsafe { end_mutation(host) };
    restored
}

fn leak_bytes(mut bytes: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    let len = bytes.len();
    if !out_len.is_null() {
        unsafe { out_len.write(len) };
    }
    if len == 0 {
        return std::ptr::null_mut();
    }
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    pointer
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_processor_state(
    state: *mut c_void,
    index: u32,
    out_len: *mut usize,
) -> *mut u8 {
    let Some(host) = (unsafe { host(state) }) else {
        return std::ptr::null_mut();
    };
    match unsafe { read_state(host, index, false, true) } {
        Some(bytes) => leak_bytes(bytes, out_len),
        None => {
            if !out_len.is_null() {
                unsafe { out_len.write(0) };
            }
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_processor_gui_state(
    state: *mut c_void,
    index: u32,
    out_len: *mut usize,
) -> *mut u8 {
    let Some(host) = (unsafe { host(state) }) else {
        return std::ptr::null_mut();
    };
    match unsafe { read_state(host, index, true, false) } {
        Some(bytes) => leak_bytes(bytes, out_len),
        None => {
            if !out_len.is_null() {
                unsafe { out_len.write(0) };
            }
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_processor_state_restore(
    state: *mut c_void,
    index: u32,
    data: *const u8,
    length: usize,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    unsafe { restore_state(host, index, data, length, false) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_processor_gui_state_restore(
    state: *mut c_void,
    index: u32,
    data: *const u8,
    length: usize,
) -> bool {
    if length > 1024 * 1024 {
        return false;
    }
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    unsafe { restore_state(host, index, data, length, true) }
}

/// Releases a buffer returned by the state accessors above.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_buffer_free(pointer: *mut u8, length: usize) {
    if pointer.is_null() || length == 0 {
        return;
    }
    drop(unsafe { Vec::from_raw_parts(pointer, length, length) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_total_latency_samples(state: *const c_void) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    unsafe { effect_chain::hirari_effect_chain_runtime_total_latency_samples(host.runtime) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_metric(state: *const c_void, metric: u32) -> u64 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    unsafe {
        effect_chain::hirari_effect_chain_runtime_metric(
            host.runtime,
            metric,
            Some(host.ops.read_metric),
        )
    }
}

/// Non-blocking diagnostic read for `noexcept` pollers. Returns false while a
/// control-thread mutation holds the chain so callers keep their last sample.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_metric_try(
    state: *const c_void,
    metric: u32,
    out_value: *mut u64,
) -> bool {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return false;
    };
    let Ok(_guard) = host.inner.try_lock() else {
        return false;
    };
    let value = unsafe {
        effect_chain::hirari_effect_chain_runtime_metric(
            host.runtime,
            metric,
            Some(host.ops.read_metric),
        )
    };
    if !out_value.is_null() {
        unsafe { out_value.write(value) };
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_reclaim(state: *mut c_void) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let mut guard = unsafe { inner(host) };
    unsafe { reclaim_locked(host, &mut guard) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_process(
    state: *mut c_void,
    user_data: *mut c_void,
    channels: *const *mut f32,
    channel_count: u32,
    parallel_channels: *const *mut f32,
    parallel_channel_count: u32,
    parallel_capacity: u32,
    frames: u32,
    process_node: Option<effect_chain::EffectNodeProcessWithHandle>,
) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    if !unsafe { effect_chain::hirari_effect_chain_runtime_enter_audio(host.runtime) } {
        if !channels.is_null() {
            for channel in 0..channel_count as usize {
                let samples = unsafe { *channels.add(channel) };
                if samples.is_null() {
                    continue;
                }
                for frame in 0..frames as usize {
                    unsafe { samples.add(frame).write(0.0) };
                }
            }
        }
        return;
    }
    unsafe {
        effect_chain::hirari_effect_chain_runtime_process_block(
            host.runtime,
            user_data,
            channels,
            channel_count,
            parallel_channels,
            parallel_channel_count,
            parallel_capacity,
            frames,
            process_node,
        );
        effect_chain::hirari_effect_chain_runtime_leave_audio(host.runtime);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    struct FakeProcessor {
        state: Vec<u8>,
        parameter0: f32,
        parameters: u32,
        latency: u32,
        tail: u64,
        pending: AtomicBool,
        prepare_sleep_micros: u64,
        prepared: AtomicU32,
        resets: AtomicU32,
    }

    impl FakeProcessor {
        fn new() -> Self {
            Self {
                state: Vec::new(),
                parameter0: 0.25,
                parameters: 0,
                latency: 7,
                tail: 11,
                pending: AtomicBool::new(true),
                prepare_sleep_micros: 0,
                prepared: AtomicU32::new(0),
                resets: AtomicU32::new(0),
            }
        }
    }

    unsafe extern "C" fn prepare(processor: *mut c_void, _sr: f64, _bs: u32) {
        let p = unsafe { &*processor.cast::<FakeProcessor>() };
        p.prepared.fetch_add(1, Ordering::SeqCst);
        if p.prepare_sleep_micros != 0 {
            std::thread::sleep(std::time::Duration::from_micros(p.prepare_sleep_micros));
        }
    }
    unsafe extern "C" fn reset(processor: *mut c_void) {
        let p = unsafe { &*processor.cast::<FakeProcessor>() };
        p.resets.fetch_add(1, Ordering::SeqCst);
    }
    unsafe extern "C" fn set_parameter(processor: *mut c_void, id: u32, value: f32) {
        if id == 0 {
            unsafe { &mut *processor.cast::<FakeProcessor>() }.parameter0 = value;
        }
    }
    unsafe extern "C" fn set_parameter_at_sample(
        processor: *mut c_void,
        id: u32,
        value: f32,
        _offset: u32,
    ) {
        unsafe { set_parameter(processor, id, value) };
    }
    unsafe extern "C" fn latency_samples(processor: *mut c_void) -> u32 {
        unsafe { &*processor.cast::<FakeProcessor>() }.latency
    }
    unsafe extern "C" fn state_size(processor: *mut c_void) -> usize {
        unsafe { &*processor.cast::<FakeProcessor>() }.state.len()
    }
    unsafe extern "C" fn state_copy(processor: *mut c_void, out: *mut u8, cap: usize) -> usize {
        let p = unsafe { &*processor.cast::<FakeProcessor>() };
        let len = p.state.len().min(cap);
        unsafe { std::ptr::copy_nonoverlapping(p.state.as_ptr(), out, len) };
        len
    }
    unsafe extern "C" fn state_restore(
        processor: *mut c_void,
        data: *const u8,
        length: usize,
    ) -> bool {
        let p = unsafe { &mut *processor.cast::<FakeProcessor>() };
        p.state = unsafe { std::slice::from_raw_parts(data, length) }.to_vec();
        true
    }
    unsafe extern "C" fn num_parameters(processor: *mut c_void) -> u32 {
        unsafe { &*processor.cast::<FakeProcessor>() }.parameters
    }
    unsafe extern "C" fn parameter(processor: *mut c_void, _id: u32) -> f32 {
        unsafe { &*processor.cast::<FakeProcessor>() }.parameter0
    }
    unsafe extern "C" fn clone_owner(owner: *mut c_void) -> *mut c_void {
        Box::into_raw(Box::new(owner)).cast()
    }
    unsafe extern "C" fn destroy_owner(_owner: *mut c_void) {}
    unsafe extern "C" fn read_metric(processor: *mut c_void, metric: u32) -> u64 {
        let p = unsafe { &*processor.cast::<FakeProcessor>() };
        match metric {
            0 => p.pending.swap(false, Ordering::SeqCst) as u64,
            2 => p.tail,
            _ => 0,
        }
    }

    fn ops() -> EffectNodeOps {
        EffectNodeOps {
            prepare,
            reset,
            set_parameter,
            set_parameter_at_sample,
            latency_samples,
            state_size,
            state_copy,
            state_restore,
            gui_state_size: state_size,
            gui_state_copy: state_copy,
            gui_state_restore: state_restore,
            num_parameters,
            parameter,
            clone_owner,
            destroy_owner,
            read_metric,
        }
    }

    fn create() -> *mut c_void {
        let ops = Box::into_raw(Box::new(ops()));
        let chain = unsafe { hirari_effect_chain_create(ops) };
        unsafe { drop(Box::from_raw(ops)) };
        chain
    }

    #[test]
    fn insert_remove_and_move_keep_order_and_latency() {
        let chain = create();
        let a = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        let b = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        let a_owner = Box::into_raw(Box::new(a));
        let b_owner = Box::into_raw(Box::new(b));
        assert!(unsafe { hirari_effect_chain_add(chain, a_owner.cast(), a, false) });
        assert!(unsafe { hirari_effect_chain_add(chain, b_owner.cast(), b, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(chain) }, 2);
        assert_eq!(unsafe { hirari_effect_chain_total_latency_samples(chain) }, 14);
        assert!(unsafe { hirari_effect_chain_move(chain, 0, 1) });
        assert_eq!(unsafe { hirari_effect_chain_processor(chain, 1) }, a);
        assert!(unsafe { hirari_effect_chain_set_bypass(chain, 0, true) });
        assert_eq!(unsafe { hirari_effect_chain_total_latency_samples(chain) }, 7);
        let mut bypassed = false;
        let mut parallel = false;
        let clone = unsafe {
            hirari_effect_chain_remove(chain, 0, &mut bypassed, &mut parallel)
        };
        assert!(bypassed);
        assert!(!parallel);
        assert!(!clone.is_null());
        unsafe { drop(Box::from_raw(clone.cast::<*mut c_void>())) };
        assert_eq!(unsafe { hirari_effect_chain_count(chain) }, 1);
        unsafe { hirari_effect_chain_destroy(chain) };
    }

    #[test]
    fn legacy_state_marker_and_restore_round_trip() {
        let chain = create();
        let processor = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        unsafe { &mut *processor.cast::<FakeProcessor>() }.parameters = 1;
        let owner = Box::into_raw(Box::new(processor));
        assert!(unsafe { hirari_effect_chain_add(chain, owner.cast(), processor, false) });

        let mut len = 0usize;
        let state = unsafe { hirari_effect_chain_processor_state(chain, 0, &mut len) };
        assert_eq!(len, std::mem::size_of::<f32>());
        let value = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        unsafe { hirari_effect_chain_buffer_free(state, len) };
        assert_eq!(f32::from_le_bytes(value.as_slice().try_into().unwrap()), 0.25);

        assert!(unsafe {
            hirari_effect_chain_processor_state_restore(chain, 0, value.as_ptr(), value.len())
        });
        let restored = unsafe { &*processor.cast::<FakeProcessor>() };
        assert_eq!(restored.state, value);
        assert!(unsafe { !hirari_effect_chain_processor_state_restore(chain, 0, std::ptr::null(), 4) });
        unsafe { hirari_effect_chain_destroy(chain) };
    }

    #[test]
    fn prepare_and_metrics_reach_every_processor() {
        let chain = create();
        let processor = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        let owner = Box::into_raw(Box::new(processor));
        assert!(unsafe { hirari_effect_chain_add(chain, owner.cast(), processor, true) });
        assert!(unsafe { hirari_effect_chain_set_sample_rate(chain, 48_000.0, 256) });
        assert!(!unsafe { hirari_effect_chain_set_sample_rate(chain, f64::NAN, 256) });
        unsafe { hirari_effect_chain_reset(chain) };
        assert_eq!(
            unsafe { &*processor.cast::<FakeProcessor>() }
                .prepared
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            unsafe { &*processor.cast::<FakeProcessor>() }
                .resets
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 1);
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 0);
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 2) }, 11);
        unsafe { hirari_effect_chain_destroy(chain) };
    }

    #[test]
    fn watchdog_trips_drain_once_per_edge() {
        let chain = create();
        let first = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        let second = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        let first_owner = Box::into_raw(Box::new(first));
        let second_owner = Box::into_raw(Box::new(second));
        assert!(unsafe { hirari_effect_chain_add(chain, first_owner.cast(), first, false) });
        assert!(unsafe { hirari_effect_chain_add(chain, second_owner.cast(), second, false) });

        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 2);
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 0);
        unsafe { &*second.cast::<FakeProcessor>() }
            .pending
            .store(true, Ordering::SeqCst);
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 1);
        assert_eq!(unsafe { hirari_effect_chain_metric(chain, 0) }, 0);
        unsafe { hirari_effect_chain_destroy(chain) };
    }

    // The engine-health overlay polls tail/non-finite diagnostics on a timer
    // from a `noexcept` reader, so a slow control-thread prepare must not block
    // it; the cached-value fallback keeps the poll bounded.
    #[test]
    fn diagnostics_never_block_on_control_thread_mutation() {
        let chain = create();
        let slow = Box::into_raw(Box::new(FakeProcessor::new())).cast::<c_void>();
        unsafe { &mut *slow.cast::<FakeProcessor>() }.prepare_sleep_micros = 20_000;
        let owner = Box::into_raw(Box::new(slow));
        assert!(unsafe { hirari_effect_chain_add(chain, owner.cast(), slow, false) });

        let address = chain as usize;
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let hog_stop = stop.clone();
        let hog = std::thread::spawn(move || {
            let chain = address as *mut c_void;
            while !hog_stop.load(Ordering::Acquire) {
                unsafe { hirari_effect_chain_set_sample_rate(chain, 48_000.0, 512) };
            }
        });

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..300 {
            let started = std::time::Instant::now();
            let mut value = 0_u64;
            unsafe {
                let _ = hirari_effect_chain_metric_try(chain, 2, &mut value);
                let _ = hirari_effect_chain_metric_try(chain, 1, &mut value);
            }
            worst = worst.max(started.elapsed());
            std::thread::yield_now();
        }
        stop.store(true, Ordering::Release);
        hog.join();
        assert!(
            worst < std::time::Duration::from_millis(5),
            "diagnostic reader blocked on the control thread: {worst:?}"
        );
        unsafe { hirari_effect_chain_destroy(chain) };
    }
}
