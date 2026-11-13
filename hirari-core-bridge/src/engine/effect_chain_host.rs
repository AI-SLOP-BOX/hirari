//! Ownership, ordering and control-side mutation for Track/Bus effect chains.
//!
//! The chain owns its nodes outright: each slot is an `Arc<ChainNode>` holding
//! either a built-in catalog host or an isolated worker (`chain::SandboxNode`).
//! The C++ engine no longer wraps processors in a vtable; it addresses slots by
//! index and this module answers with the same node it stores. Snapshot
//! publication and the realtime reader rules live in `effect_chain.rs`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio_buffer_ops::{
    hirari_audio_buffer_storage_channel_pointers, hirari_audio_buffer_storage_channels,
    hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
    hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_samples,
};
use crate::effect_chain::{self, RuntimeSnapshot};
use crate::effect_chain_nodes::{BlockAudio, ChainNode, SANDBOX_KIND};
use crate::internal_plugin_catalog::hirari_internal_plugin_path;
use crate::midi_buffer_ops::MidiView;
use crate::plugin_admission::hirari_plugin_admission_error;
use crate::plugin_blacklist::{
    hirari_plugin_scan_cache_record_failure, hirari_plugin_scan_cache_record_success,
};
use crate::plugin_sandbox_host::chain::SandboxNode;
use crate::plugin_sandbox_maintenance::hirari_plugin_sandbox_transport_retry_delay;
use crate::plugin_sandbox_map::{
    hirari_sandbox_slots_contains_path, hirari_sandbox_slots_create, hirari_sandbox_slots_destroy,
    hirari_sandbox_slots_index, hirari_sandbox_slots_insert, hirari_sandbox_slots_len,
    hirari_sandbox_slots_move, hirari_sandbox_slots_path_copy, hirari_sandbox_slots_path_size,
    hirari_sandbox_slots_push, hirari_sandbox_slots_remove, SandboxSlotStore,
};

/// Mirrors `HirariTimedParameterEvent` on the C++ side.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimedParameterEvent {
    pub processor_index: u32,
    pub parameter_id: u32,
    pub sample_offset: u32,
    pub normalized_value: f32,
}

/// Parallel-send scratch the traversal mixes back into the dry block.
const PARALLEL_CHANNELS: u32 = 2;
const PARALLEL_FRAMES: u32 = 4096;

/// The plugin types the built-in catalog resolves.
const MAX_BUILTIN_TYPE: u32 = 10;

/// Reason text for the last sandbox edit a control caller may read. Every
/// entry is a static NUL-terminated string, so a published pointer stays valid
/// for the whole process and the accessor never needs the chain lock.
static TEXT_INVALID_PATH: &[u8] = b"invalid-plugin-path\0";
static TEXT_PATH_MISSING: &[u8] = b"plugin-path-missing\0";
static TEXT_FORMAT_UNSUPPORTED: &[u8] = b"plugin-format-unsupported\0";
static TEXT_DUPLICATE_PATH: &[u8] = b"duplicate-plugin-path\0";
static TEXT_START_FAILED: &[u8] = b"sandbox-start-failed\0";
static TEXT_NONE: &[u8] = b"none\0";

/// Publishes one of the static reason strings as the last-failure text.
fn text_pointer(text: &'static [u8]) -> *mut c_char {
    text.as_ptr().cast::<c_char>().cast_mut()
}

/// `STATE_ERROR_UNAVAILABLE`: no worker to ask about.
const STATE_ERROR_UNAVAILABLE: u8 = 5;

/// Transport-retry budget applied around worker start.
const MAX_TRANSPORT_RETRIES: u32 = 5;

struct Entry {
    node: Arc<ChainNode>,
    bypassed: bool,
    parallel: bool,
    /// Built-in plugin type, or `SANDBOX_KIND` for an isolated worker. The entry
    /// carries its own identity so ordering cannot drift from the node.
    kind: u32,
    /// Worker path kept for the undo slot permutation.
    sandbox_path: Vec<u8>,
}

struct HostInner {
    entries: Vec<Entry>,
    /// Removed nodes kept alive until every audio reader has drained.
    retired: Vec<Arc<ChainNode>>,
    /// Nodes taken out for an undo entry, addressable by the caller's token.
    detached: Vec<(u32, Entry)>,
    next_detach_token: u32,
    sample_rate: f64,
    block_size: u32,
    slots: *mut SandboxSlotStore,
}

// The slot store and the scratch planes are only touched under this mutex.
unsafe impl Send for HostInner {}

pub struct EffectChainHost {
    runtime: *mut c_void,
    inner: Mutex<HostInner>,
    /// Parallel wet/dry scratch, allocated once at creation and never resized,
    /// so the audio thread reads its channel table without the control mutex.
    parallel_storage: *mut c_void,
    /// Read on the audio thread by the built-in-voice gate, so it must not
    /// require the control mutex.
    midi_instrument_count: AtomicU32,
    last_sandbox_failure: AtomicU8,
    last_sandbox_text: AtomicPtr<c_char>,
    /// Last value of the polled non-finite diagnostic, kept outside the control
    /// mutex so a contended poll still reports a measurement.
    last_non_finite: AtomicU64,
}

/// One worker's diagnostic snapshot, laid out to match the engine's struct.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SandboxStatusOut {
    pub plugin_index: u32,
    pub alive: u8,
    pub can_retry: u8,
    pub failure: u8,
    pub recovery_mode: u8,
    pub dropped_output_midi: u32,
    pub mailbox_overruns: u32,
    pub input_midi_truncations: u32,
}

unsafe fn host<'a>(state: *mut c_void) -> Option<&'a EffectChainHost> {
    unsafe { state.cast::<EffectChainHost>().as_ref() }
}

fn inner_ref(host: &EffectChainHost) -> std::sync::MutexGuard<'_, HostInner> {
    host.inner.lock().unwrap_or_else(|error| error.into_inner())
}

fn sync_instrument_count(host: &EffectChainHost, inner: &HostInner) {
    let count = inner
        .entries
        .iter()
        .filter(|entry| entry.node.is_midi_instrument())
        .count() as u32;
    host.midi_instrument_count.store(count, Ordering::Release);
}

unsafe fn publish(host: &EffectChainHost, inner: &HostInner) {
    let mut views = Vec::with_capacity(inner.entries.len());
    for entry in &inner.entries {
        views.push(effect_chain::EffectChainNodeView {
            processor: Arc::as_ptr(&entry.node).cast_mut().cast(),
            bypassed: entry.bypassed as u8,
            parallel: entry.parallel as u8,
            latency_samples: entry.node.latency_samples(),
        });
    }
    unsafe {
        effect_chain::hirari_effect_chain_runtime_publish(host.runtime, views.as_ptr(), views.len())
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

/// Clones the node out of the control lock so a worker call never runs while
/// the chain mutex is held.
fn entry_node(host: &EffectChainHost, index: u32) -> Option<Arc<ChainNode>> {
    inner_ref(host)
        .entries
        .get(index as usize)
        .map(|entry| Arc::clone(&entry.node))
}

/// Resolves a sandbox slot to its node. A slot whose chain entry was removed or
/// reordered out from under it answers with none.
fn sandbox_slot(host: &EffectChainHost, slot: usize) -> Option<Arc<ChainNode>> {
    let guard = inner_ref(host);
    let index = unsafe { hirari_sandbox_slots_index(guard.slots as *const _, slot) };
    let entry = guard.entries.get(index as usize)?;
    if entry.kind != SANDBOX_KIND {
        return None;
    }
    Some(Arc::clone(&entry.node))
}

fn slot_path(inner: &HostInner, slot: usize) -> Vec<u8> {
    let length = unsafe { hirari_sandbox_slots_path_size(inner.slots as *const _, slot) };
    let mut path = vec![0_u8; length];
    if length == 0 {
        return path;
    }
    if unsafe {
        hirari_sandbox_slots_path_copy(inner.slots as *const _, slot, path.as_mut_ptr(), length)
    } != length
    {
        path.clear();
    }
    path
}

unsafe fn reclaim_locked(host: &EffectChainHost, inner: &mut HostInner) {
    if unsafe { reader_count(host) } != 0 {
        return;
    }
    unsafe { effect_chain::hirari_effect_chain_runtime_reclaim(host.runtime) };
    inner.retired.clear();
}

#[no_mangle]
pub extern "C" fn hirari_effect_chain_create() -> *mut c_void {
    let runtime = effect_chain::hirari_effect_chain_runtime_create();
    if runtime.is_null() {
        return std::ptr::null_mut();
    }
    let slots = hirari_sandbox_slots_create();
    let parallel_storage = hirari_audio_buffer_storage_create();
    if slots.is_null() || parallel_storage.is_null() {
        unsafe {
            hirari_sandbox_slots_destroy(slots);
            hirari_audio_buffer_storage_destroy(parallel_storage);
            effect_chain::hirari_effect_chain_runtime_destroy(runtime);
        }
        return std::ptr::null_mut();
    }
    // Sized once here, exactly as the C++ chain preallocated its parallel
    // scratch, so no control-thread resize can race an audio callback that is
    // reading the channel pointers.
    let ready = unsafe {
        hirari_audio_buffer_storage_reserve_buffer(parallel_storage, PARALLEL_CHANNELS, PARALLEL_FRAMES)
            == 0
            && hirari_audio_buffer_storage_resize(parallel_storage, PARALLEL_CHANNELS, PARALLEL_FRAMES)
    };
    if !ready {
        unsafe {
            hirari_sandbox_slots_destroy(slots);
            hirari_audio_buffer_storage_destroy(parallel_storage);
            effect_chain::hirari_effect_chain_runtime_destroy(runtime);
        }
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(EffectChainHost {
        runtime,
        parallel_storage,
        inner: Mutex::new(HostInner {
            entries: Vec::new(),
            retired: Vec::new(),
            detached: Vec::new(),
            next_detach_token: 1,
            sample_rate: 0.0,
            block_size: 0,
            slots,
        }),
        midi_instrument_count: AtomicU32::new(0),
        last_sandbox_failure: AtomicU8::new(0),
        last_sandbox_text: AtomicPtr::new(text_pointer(TEXT_NONE)),
        last_non_finite: AtomicU64::new(0),
    }))
    .cast()
}

/// Waits for audio readers, then releases every node the chain owns. A callback
/// that cannot quiesce within the bounded wait leaks the whole chain rather than
/// freeing memory a realtime thread may still traverse.
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
    let EffectChainHost { runtime, inner, parallel_storage, .. } = *host;
    let inner = inner.into_inner().unwrap_or_else(|error| error.into_inner());
    unsafe {
        hirari_sandbox_slots_destroy(inner.slots);
        hirari_audio_buffer_storage_destroy(parallel_storage);
        effect_chain::hirari_effect_chain_runtime_destroy(runtime);
    }
    // Nodes drop here, which stops each worker and destroys each native host.
    drop(inner);
}

fn builtin_node(plugin_type: u32, bus: bool) -> Option<Arc<ChainNode>> {
    if plugin_type > MAX_BUILTIN_TYPE {
        return None;
    }
    let path = hirari_internal_plugin_path(plugin_type, bus);
    if path.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    ChainNode::internal(bytes).map(Arc::from)
}

/// Adds a built-in catalog processor. UI/Message thread only.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_add_builtin(
    state: *mut c_void,
    plugin_type: u32,
    bus: bool,
    parallel: bool,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let Some(node) = builtin_node(plugin_type, bus) else {
        return false;
    };
    let mut guard = inner_ref(host);
    if guard.sample_rate > 0.0 && guard.block_size > 0 {
        node.prepare(guard.sample_rate, guard.block_size);
    }
    guard.entries.push(Entry {
        node,
        bypassed: false,
        parallel,
        kind: plugin_type,
        sandbox_path: Vec::new(),
    });
    unsafe {
        publish(host, &guard);
    }
    sync_instrument_count(host, &guard);
    true
}

/// Inserts an isolated worker, applying the admission, duplicate-path and
/// transport-retry policy that used to sit in the engine's track wrapper.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_add_sandbox(
    state: *mut c_void,
    path: *const u8,
    path_len: usize,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let reject = |host: &EffectChainHost, text: &'static [u8]| {
        host.last_sandbox_failure.store(0, Ordering::Release);
        host.last_sandbox_text.store(text_pointer(text), Ordering::Release);
        false
    };
    if path.is_null() || path_len == 0 {
        return reject(host, TEXT_INVALID_PATH);
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, path_len) };
    let Ok(cpath) = CString::new(bytes.to_vec()) else {
        return reject(host, TEXT_INVALID_PATH);
    };
    match unsafe { hirari_plugin_admission_error(cpath.as_ptr()) } {
        1 => return reject(host, TEXT_INVALID_PATH),
        2 => return reject(host, TEXT_PATH_MISSING),
        3 => return reject(host, TEXT_FORMAT_UNSUPPORTED),
        _ => {}
    }
    let guard = inner_ref(host);
    if unsafe { hirari_sandbox_slots_contains_path(guard.slots as *const _, path, path_len) } {
        drop(guard);
        return reject(host, TEXT_DUPLICATE_PATH);
    }
    let (sample_rate, block_size) = (guard.sample_rate, guard.block_size.max(1));
    drop(guard);

    // The transport-retry policy (which failures are transient and the backoff)
    // lives here; close the failed worker before a retry.
    let node = SandboxNode::new(bytes, sample_rate, block_size);
    let mut started = node.start();
    for attempt in 0..MAX_TRANSPORT_RETRIES {
        if started || attempt + 1 >= MAX_TRANSPORT_RETRIES {
            break;
        }
        let delay = hirari_plugin_sandbox_transport_retry_delay(attempt, node.failure_code());
        if delay == 0 {
            break;
        }
        node.stop();
        std::thread::sleep(Duration::from_millis(u64::from(delay)));
        started = node.start();
    }
    if !started {
        let failure = node.failure_code();
        host.last_sandbox_failure.store(failure, Ordering::Release);
        host.last_sandbox_text.store(
            if failure == 0 {
                text_pointer(TEXT_START_FAILED)
            } else {
                node.failure_text().cast_mut()
            },
            Ordering::Release,
        );
        unsafe {
            hirari_plugin_scan_cache_record_failure(path, path_len, u32::from(failure));
        }
        return false;
    }
    unsafe {
        hirari_plugin_scan_cache_record_success(path, path_len);
    }
    host.last_sandbox_failure.store(0, Ordering::Release);
    host.last_sandbox_text.store(text_pointer(TEXT_NONE), Ordering::Release);
    let chain_node: Arc<ChainNode> = Arc::from(ChainNode::sandbox(node));
    if sample_rate > 0.0 && block_size > 0 {
        chain_node.prepare(sample_rate, block_size);
    }
    let mut guard = inner_ref(host);
    let index = guard.entries.len() as u32;
    guard.entries.push(Entry {
        node: chain_node,
        bypassed: false,
        parallel: false,
        kind: SANDBOX_KIND,
        sandbox_path: bytes.to_vec(),
    });
    unsafe {
        hirari_sandbox_slots_push(guard.slots, index, path, path_len);
        publish(host, &guard);
    }
    sync_instrument_count(host, &guard);
    true
}

/// Removes one entry and issues a token addressing the detached node. The chain
/// keeps the node alive until it is re-inserted or the chain is destroyed, so an
/// undo entry only has to remember the token.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_remove(
    state: *mut c_void,
    index: u32,
    bypassed: *mut bool,
    parallel: *mut bool,
    kind: *mut u32,
    token: *mut u32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let mut guard = inner_ref(host);
    if index as usize >= guard.entries.len() {
        return false;
    }
    let entry = guard.entries.remove(index as usize);
    if !bypassed.is_null() {
        unsafe { bypassed.write(entry.bypassed) };
    }
    if !parallel.is_null() {
        unsafe { parallel.write(entry.parallel) };
    }
    if !kind.is_null() {
        unsafe { kind.write(entry.kind) };
    }
    let issued = guard.next_detach_token;
    guard.next_detach_token = guard.next_detach_token.wrapping_add(1);
    guard.detached.push((issued, entry));
    unsafe {
        hirari_sandbox_slots_remove(guard.slots, index);
        publish(host, &guard);
    }
    sync_instrument_count(host, &guard);
    unsafe { reclaim_locked(host, &mut guard) };
    if !token.is_null() {
        unsafe { token.write(issued) };
    }
    true
}

/// Re-inserts the exact node captured by [`hirari_effect_chain_remove`],
/// preserving external-plugin state and sandbox identity.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_restore(
    state: *mut c_void,
    index: u32,
    token: u32,
    bypassed: bool,
    parallel: bool,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let mut guard = inner_ref(host);
    let Some(position) = guard.detached.iter().position(|(id, _)| *id == token) else {
        return false;
    };
    let (_, mut entry) = guard.detached.remove(position);
    entry.bypassed = bypassed;
    entry.parallel = parallel;
    if guard.sample_rate > 0.0 && guard.block_size > 0 {
        entry.node.prepare(guard.sample_rate, guard.block_size);
    }
    let position = (index as usize).min(guard.entries.len());
    let is_sandbox = entry.kind == SANDBOX_KIND;
    let path = entry.sandbox_path.clone();
    guard.entries.insert(position, entry);
    if is_sandbox && !path.is_empty() {
        unsafe {
            hirari_sandbox_slots_insert(guard.slots, position as u32, path.as_ptr(), path.len());
        }
    }
    unsafe { publish(host, &guard) };
    sync_instrument_count(host, &guard);
    true
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
    let mut guard = inner_ref(host);
    let count = guard.entries.len();
    if from as usize >= count || to as usize >= count || from == to {
        return false;
    }
    let entry = guard.entries.remove(from as usize);
    guard.entries.insert(to as usize, entry);
    unsafe {
        hirari_sandbox_slots_move(guard.slots, from, to);
        publish(host, &guard);
    }
    true
}

/// Drops every entry, keeping the nodes alive until the audio readers drain,
/// and empties the sandbox identity list with them. The engine rebuilds a
/// chain by re-adding nodes, so this is only reachable from the chain's tests.
unsafe fn clear(state: *mut c_void) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let mut guard = inner_ref(host);
    while unsafe { hirari_sandbox_slots_len(guard.slots as *const _) } != 0 {
        unsafe { hirari_sandbox_slots_remove(guard.slots, 0) };
    }
    let cleared = std::mem::take(&mut guard.entries);
    guard
        .retired
        .extend(cleared.into_iter().map(|entry| entry.node));
    unsafe { publish(host, &guard) };
    sync_instrument_count(host, &guard);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_count(state: *const c_void) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    inner_ref(host).entries.len() as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_node_kind(state: *const c_void, index: u32) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    inner_ref(host)
        .entries
        .get(index as usize)
        .map_or(0, |entry| entry.kind)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_midi_instrument_count(state: *const c_void) -> u32 {
    unsafe { host(state.cast_mut()) }
        .map_or(0, |host| host.midi_instrument_count.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_last_sandbox_failure(state: *const c_void) -> u8 {
    unsafe { host(state.cast_mut()) }
        .map_or(0, |host| host.last_sandbox_failure.load(Ordering::Acquire))
}

/// A worker that is currently reporting a fault wins over the last edit text.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_last_sandbox_failure_text(
    state: *const c_void,
) -> *const c_char {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return std::ptr::null();
    };
    let count = {
        let guard = inner_ref(host);
        unsafe { hirari_sandbox_slots_len(guard.slots as *const _) }
    };
    for slot in 0..count {
        if let Some(node) = sandbox_slot(host, slot) {
            if let Some(sandbox) = node.sandbox_node() {
                if sandbox.failure_code() != 0 {
                    return sandbox.failure_text();
                }
            }
        }
    }
    host.last_sandbox_text.load(Ordering::Acquire).cast_const()
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
    let mut guard = inner_ref(host);
    let Some(entry) = guard.entries.get_mut(index as usize) else {
        return false;
    };
    entry.bypassed = bypassed;
    unsafe { publish(host, &guard) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_bypass(state: *const c_void, index: u32) -> bool {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return false;
    };
    inner_ref(host)
        .entries
        .get(index as usize)
        .is_some_and(|entry| entry.bypassed)
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
    let mut guard = inner_ref(host);
    unsafe { begin_mutation(host) };
    guard.sample_rate = sample_rate;
    guard.block_size = block_size;
    for entry in &guard.entries {
        entry.node.prepare(sample_rate, block_size);
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
    let guard = inner_ref(host);
    unsafe { begin_mutation(host) };
    for entry in &guard.entries {
        entry.node.reset();
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
    let Some(node) = entry_node(host, index) else {
        return false;
    };
    unsafe { begin_mutation(host) };
    node.set_parameter(parameter_id, value.clamp(0.0, 1.0));
    unsafe { end_mutation(host) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_parameter(
    state: *const c_void,
    index: u32,
    parameter_id: u32,
) -> f32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0.0;
    };
    entry_node(host, index).map_or(0.0, |node| node.parameter(parameter_id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_parameter_count(
    state: *const c_void,
    index: u32,
) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    entry_node(host, index).map_or(0, |node| node.num_parameters())
}

/// Display name for one sandbox parameter, from the discovery probe cache.
#[path = "effect_chain_host_ffi.rs"]
mod ffi;
pub use ffi::*;

#[cfg(test)]
#[path = "effect_chain_host_tests.rs"]
mod effect_chain_host_tests;
