//! Ownership, ordering and control-side mutation for Track/Bus effect chains.
//!
//! The chain owns its nodes outright: each slot is an `Arc<ChainNode>` holding
//! either a built-in catalog host or an isolated worker (`chain::SandboxNode`).
//! The C++ engine no longer wraps processors in a vtable; it addresses slots by
//! index and this module answers with the same node it stores. Snapshot
//! publication and the realtime reader rules live in `effect_chain.rs`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU8, Ordering};
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
    // The published snapshot is the same node list the traversal walks, so
    // automation lands on the processors this block actually hears. Addressed
    // through it, the realtime path allocates nothing.
    let snapshot = unsafe { effect_chain::audio_snapshot(host.runtime) };
    if !snapshot.is_null() {
        let events = unsafe { std::slice::from_raw_parts(events, event_count) };
        for event in events {
            if !event.normalized_value.is_finite() {
                continue;
            }
            let Some(node) = (unsafe { &*snapshot })
                .nodes
                .get(event.processor_index as usize)
                .and_then(|entry| unsafe { entry.node.cast::<ChainNode>().as_ref() })
            else {
                continue;
            };
            node.set_parameter_at_sample(event.parameter_id, event.normalized_value, event.sample_offset);
        }
    }
    unsafe { effect_chain::hirari_effect_chain_runtime_leave_audio(host.runtime) };
}

unsafe fn read_state(host: &EffectChainHost, index: u32, gui: bool) -> Option<Vec<u8>> {
    let node = entry_node(host, index)?;
    Some(if gui { node.gui_state() } else { node.state() })
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
    let Some(node) = entry_node(host, index) else {
        return false;
    };
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    // A project load restores while the audio thread reads the same nodes.
    unsafe { begin_mutation(host) };
    let restored = if gui {
        node.restore_gui_state(bytes)
    } else {
        node.restore_state(bytes)
    };
    unsafe { end_mutation(host) };
    restored
}

/// Hands a state buffer to a C++ caller. The allocation must have exact
/// capacity so `hirari_effect_chain_buffer_free` can rebuild a boxed slice from
/// the pointer and length alone.
fn leak_bytes(bytes: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    let mut boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    if !out_len.is_null() {
        unsafe { out_len.write(len) };
    }
    if len == 0 {
        return std::ptr::null_mut();
    }
    let pointer = boxed.as_mut_ptr();
    std::mem::forget(boxed);
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
    match unsafe { read_state(host, index, false) } {
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
    match unsafe { read_state(host, index, true) } {
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
    drop(unsafe { Box::<[u8]>::from_raw(std::slice::from_raw_parts_mut(pointer, length)) });
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
    let guard = inner_ref(host);
    node_metric(&guard, metric)
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
    let Ok(guard) = host.inner.try_lock() else {
        return false;
    };
    let value = node_metric(&guard, metric);
    if !out_value.is_null() {
        unsafe { out_value.write(value) };
    }
    true
}

/// Folds node diagnostics and effect tails over the current chain order. Metric
/// 2 is the serial tail plus the longest parallel branch; the rest sum.
fn node_metric(inner: &HostInner, metric: u32) -> u64 {
    let mut total = 0_u64;
    let mut serial_tail = 0_u64;
    let mut parallel_tail = 0_u32;
    for entry in &inner.entries {
        let value = entry.node.metric(metric);
        match metric {
            0 | 1 => total = total.wrapping_add(value),
            2 if !entry.bypassed => {
                let tail = value as u32;
                if entry.parallel {
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

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_reclaim(state: *mut c_void) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let mut guard = inner_ref(host);
    unsafe { reclaim_locked(host, &mut guard) };
}

struct RealtimeContext {
    main: BlockAudio,
    parallel: BlockAudio,
    snapshot: *const RuntimeSnapshot,
    midi: *mut c_void,
    bpm: f64,
}

unsafe extern "C" fn realtime_node_info(
    user_data: *mut c_void,
    index: u32,
    bypassed: *mut bool,
    parallel: *mut bool,
) -> bool {
    let Some(context) = (unsafe { user_data.cast::<RealtimeContext>().as_ref() }) else {
        return false;
    };
    if context.snapshot.is_null() {
        return false;
    }
    let Some(node) = (unsafe { &*context.snapshot }).nodes.get(index as usize) else {
        return false;
    };
    unsafe {
        bypassed.write(node.bypassed);
        parallel.write(node.parallel);
    }
    true
}

unsafe extern "C" fn realtime_process_node(
    user_data: *mut c_void,
    index: u32,
    parallel: bool,
    mix: *mut f32,
) -> bool {
    let Some(context) = (unsafe { user_data.cast::<RealtimeContext>().as_ref() }) else {
        return false;
    };
    if context.snapshot.is_null() {
        return false;
    }
    let Some(node) = (unsafe { &*context.snapshot }).nodes.get(index as usize) else {
        return false;
    };
    let Some(chain) = (unsafe { node.node.cast::<ChainNode>().as_ref() }) else {
        return false;
    };
    unsafe { mix.write(chain.mix()) };
    chain.process(
        if parallel {
            &context.parallel
        } else {
            &context.main
        },
        context.bpm,
        context.midi,
    )
}

/// Runs the published node list over one buffer. `frames` and the channel table
/// come from the caller's own storage, exactly as the C++ callback did.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_process(
    state: *mut c_void,
    audio_storage: *mut c_void,
    midi_state: *mut c_void,
    bpm: f64,
) {
    let Some(host) = (unsafe { host(state) }) else {
        return;
    };
    let channels =
        unsafe { hirari_audio_buffer_storage_channel_pointers(audio_storage as *const _) };
    let channel_count = unsafe { hirari_audio_buffer_storage_channels(audio_storage as *const _) };
    let frames = unsafe { hirari_audio_buffer_storage_samples(audio_storage as *const _) };
    let main = BlockAudio::new(channels, channel_count, frames);
    if !unsafe { effect_chain::hirari_effect_chain_runtime_enter_audio(host.runtime) } {
        main.clear();
        return;
    }
    let parallel = BlockAudio::new(
        unsafe { hirari_audio_buffer_storage_channel_pointers(host.parallel_storage as *const _) },
        PARALLEL_CHANNELS,
        PARALLEL_FRAMES,
    );
    let context = RealtimeContext {
        main,
        parallel,
        snapshot: unsafe { effect_chain::audio_snapshot(host.runtime) },
        midi: midi_state,
        bpm,
    };
    let node_count = if context.snapshot.is_null() {
        0
    } else {
        unsafe { (&*context.snapshot).nodes.len() as u32 }
    };
    unsafe {
        effect_chain::hirari_effect_chain_process_block(
            (&context as *const RealtimeContext).cast_mut().cast(),
            node_count,
            channels,
            channel_count,
            parallel.pointer_array(),
            parallel.channels(),
            parallel.samples(),
            frames,
            Some(realtime_node_info),
            Some(realtime_process_node),
        );
        effect_chain::hirari_effect_chain_runtime_leave_audio(host.runtime);
    }
}

/// Chain-owned sandbox slot list. Slot `i` addresses the chain index recorded
/// when the worker was added, so removal and reordering carry it along.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_slot_count(state: *const c_void) -> usize {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    let guard = inner_ref(host);
    unsafe { hirari_sandbox_slots_len(guard.slots as *const _) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_slot_index(
    state: *const c_void,
    slot: usize,
) -> u32 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return u32::MAX;
    };
    let guard = inner_ref(host);
    unsafe { hirari_sandbox_slots_index(guard.slots as *const _, slot) }
}

/// Copies one slot's plugin path. Returns its length whether or not it fitted,
/// so a caller can size a buffer with a null `out` probe.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_slot_path(
    state: *const c_void,
    slot: usize,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    let guard = inner_ref(host);
    let length = unsafe { hirari_sandbox_slots_path_size(guard.slots as *const _, slot) };
    if out.is_null() || length > capacity {
        return length;
    }
    unsafe { hirari_sandbox_slots_path_copy(guard.slots as *const _, slot, out, capacity) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_status(
    state: *const c_void,
    slot: usize,
    out: *mut SandboxStatusOut,
) -> bool {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return false;
    };
    if out.is_null() {
        return false;
    };
    let Some(node) = sandbox_slot(host, slot) else {
        return false;
    };
    let Some(sandbox) = node.sandbox_node() else {
        return false;
    };
    let index = {
        let guard = inner_ref(host);
        unsafe { hirari_sandbox_slots_index(guard.slots as *const _, slot) }
    };
    unsafe {
        out.write(SandboxStatusOut {
            plugin_index: index,
            alive: u8::from(sandbox.is_alive()),
            can_retry: u8::from(sandbox.can_retry()),
            failure: sandbox.failure_code(),
            recovery_mode: u8::from(sandbox.recovery_failed()),
            dropped_output_midi: sandbox.take_dropped_output_midi(),
            mailbox_overruns: sandbox.take_mailbox_overruns(),
            input_midi_truncations: sandbox.take_input_midi_truncations(),
        });
    }
    true
}

/// Restarts one worker. `record_success` distinguishes the explicit user
/// restart from the retry path, which also clears the path's scan-cache fault.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_restart(
    state: *mut c_void,
    slot: usize,
    force: bool,
    record_success: bool,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    let Some(node) = sandbox_slot(host, slot) else {
        return false;
    };
    let Some(sandbox) = node.sandbox_node() else {
        return false;
    };
    if !sandbox.restart(force) {
        return false;
    }
    if record_success {
        let path = {
            let guard = inner_ref(host);
            slot_path(&guard, slot)
        };
        unsafe {
            hirari_plugin_scan_cache_record_success(path.as_ptr(), path.len());
        }
    }
    true
}

/// Polls every worker and restarts the unhealthy ones the caller allows,
/// clearing each recovered path's scan-cache fault exactly as the engine's
/// maintenance pass did.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_maintain(
    state: *mut c_void,
    auto_restart: bool,
) -> u32 {
    let Some(host) = (unsafe { host(state) }) else {
        return 0;
    };
    let mut recovered = 0;
    for slot in 0..unsafe { hirari_effect_chain_sandbox_slot_count(state as *const _) } {
        let Some(node) = sandbox_slot(host, slot) else {
            continue;
        };
        let Some(sandbox) = node.sandbox_node() else {
            continue;
        };
        if sandbox.poll_health() {
            continue;
        }
        if auto_restart && sandbox.can_auto_retry() && sandbox.auto_restart() {
            let path = {
                let guard = inner_ref(host);
                slot_path(&guard, slot)
            };
            unsafe {
                hirari_plugin_scan_cache_record_success(path.as_ptr(), path.len());
            }
            recovered += 1;
        }
    }
    recovered
}

/// Answers a control caller has for a slot with no live worker.
static STATE_ERROR_UNAVAILABLE_TEXT: &[u8] = b"state-unavailable\0";

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_state_error(
    state: *const c_void,
    slot: usize,
) -> u8 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return STATE_ERROR_UNAVAILABLE;
    };
    let Some(node) = sandbox_slot(host, slot) else {
        return STATE_ERROR_UNAVAILABLE;
    };
    let Some(sandbox) = node.sandbox_node() else {
        return STATE_ERROR_UNAVAILABLE;
    };
    sandbox.state_error_code()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_sandbox_state_error_text(
    state: *const c_void,
    slot: usize,
) -> *const c_char {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return STATE_ERROR_UNAVAILABLE_TEXT.as_ptr().cast();
    };
    let Some(node) = sandbox_slot(host, slot) else {
        return STATE_ERROR_UNAVAILABLE_TEXT.as_ptr().cast();
    };
    let Some(sandbox) = node.sandbox_node() else {
        return STATE_ERROR_UNAVAILABLE_TEXT.as_ptr().cast();
    };
    sandbox.state_error_text()
}

/// Control/test boundary for validating an isolated worker with a caller-owned
/// block. The normal transport still uses the chain traversal.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_process_sandbox_block(
    state: *mut c_void,
    slot: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else {
        return false;
    };
    if left.is_null() || right.is_null() || frames == 0 || frames > SandboxNode::max_frames() {
        return false;
    }
    let Some(node) = sandbox_slot(host, slot) else {
        return false;
    };
    let planes = [left, right];
    let audio = BlockAudio::new(planes.as_ptr(), 2, frames);
    let midi_state = crate::midi_buffer_ops::hirari_midi_buffer_create();
    let produced = node.process(&audio, 0.0, midi_state);
    unsafe { crate::midi_buffer_ops::hirari_midi_buffer_destroy(midi_state) };
    produced
}

/// Runs one worker block with a caller-supplied MIDI event and returns its last
/// output event as a leaked buffer, freed by `hirari_effect_chain_buffer_free`.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_process_sandbox_midi_block(
    state: *mut c_void,
    slot: usize,
    frames: u32,
    midi_data: *const u8,
    midi_size: usize,
    out_len: *mut usize,
) -> *mut u8 {
    let Some(host) = (unsafe { host(state) }) else {
        return std::ptr::null_mut();
    };
    if midi_data.is_null() || midi_size == 0 || frames == 0 || frames > SandboxNode::max_frames() {
        return std::ptr::null_mut();
    }
    let Some(node) = sandbox_slot(host, slot) else {
        return std::ptr::null_mut();
    };
    let midi_state = crate::midi_buffer_ops::hirari_midi_buffer_create();
    let mut midi = MidiView::new(midi_state);
    if midi_size > SandboxNode::max_payload_bytes() {
        if let Some(sandbox) = node.sandbox_node() {
            sandbox.record_input_midi_rejection();
        }
        unsafe { crate::midi_buffer_ops::hirari_midi_buffer_destroy(midi_state) };
        return std::ptr::null_mut();
    }
    let data = unsafe { std::slice::from_raw_parts(midi_data, midi_size) };
    midi.add(0, data, 0);
    let mut scratch = vec![0_f32; frames as usize * 2];
    let planes = [scratch.as_mut_ptr(), unsafe {
        scratch.as_mut_ptr().add(frames as usize)
    }];
    let audio = BlockAudio::new(planes.as_ptr(), 2, frames);
    let produced = node.process(&audio, 0.0, midi_state);
    let last = if produced && midi.len() >= 2 {
        midi.event(midi.len() - 1)
            .map(|(_, _, bytes)| bytes.to_vec())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    unsafe { crate::midi_buffer_ops::hirari_midi_buffer_destroy(midi_state) };
    leak_bytes(last, out_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_buffer_ops::{
        hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_reserve_buffer,
    };

    fn chain() -> *mut c_void {
        unsafe { hirari_effect_chain_create() }
    }

    #[test]
    fn builtins_keep_order_kind_and_bypass() {
        let state = chain();
        assert!(!state.is_null());
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 4, false, false) });
        assert!(!unsafe { hirari_effect_chain_add_builtin(state, 99, false, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 2);
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 0) },
            1
        );
        assert!(unsafe { hirari_effect_chain_move(state, 0, 1) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 1) },
            1
        );
        assert!(unsafe { hirari_effect_chain_set_bypass(state, 0, true) });
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 0) });
        assert_eq!(
            unsafe { hirari_effect_chain_midi_instrument_count(state as *const _) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_sandbox_slot_count(state as *const _) },
            0
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn the_bus_flag_only_diverges_for_the_limiter_slot() {
        let state = chain();
        // Slot 0 asks the catalog for Hirari/BusLimiter, which no DSP implements
        // yet, so a bus limiter insert is refused rather than silently inserted.
        assert!(!unsafe { hirari_effect_chain_add_builtin(state, 0, true, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 0);
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 0, false, false) });
        // Every later slot resolves to the same path for a bus and a track.
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, true, true) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, false, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 3);
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 1) == false });
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn removed_node_returns_through_a_detach_token() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, false, false) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 3, false, true) });
        let mut bypassed = true;
        let mut parallel = false;
        let mut kind = 0;
        let mut token = 0;
        assert!(unsafe {
            hirari_effect_chain_remove(
                state,
                1,
                &mut bypassed,
                &mut parallel,
                &mut kind,
                &mut token,
            )
        });
        assert_eq!(kind, 3);
        assert!(parallel);
        assert!(!bypassed);
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 1);
        assert!(unsafe { hirari_effect_chain_restore(state, 1, token, true, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 2);
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 1) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 1) },
            3
        );
        assert!(!unsafe { hirari_effect_chain_restore(state, 0, token + 500, false, false) });
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn parameters_and_state_round_trip_through_a_builtin_node() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        let count = unsafe { hirari_effect_chain_parameter_count(state as *const _, 0) };
        assert!(count > 0);
        assert!(unsafe { hirari_effect_chain_set_parameter(state, 0, 0, 0.75) });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.75
        );
        assert!(!unsafe { hirari_effect_chain_set_parameter(state, 0, 0, f32::NAN) });
        let mut length = 0;
        let bytes = unsafe { hirari_effect_chain_processor_state(state, 0, &mut length) };
        assert!(!bytes.is_null());
        assert!(length > 0);
        let saved = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
        unsafe { hirari_effect_chain_buffer_free(bytes, length) };
        assert!(unsafe { hirari_effect_chain_set_parameter(state, 0, 0, 0.1) });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.1
        );
        assert!(unsafe {
            hirari_effect_chain_processor_state_restore(state, 0, saved.as_ptr(), saved.len())
        });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.75
        );
        unsafe { hirari_effect_chain_reset(state) };
        let mut gui_len = 0usize;
        assert!(
            unsafe { hirari_effect_chain_processor_gui_state(state, 0, &mut gui_len) }.is_null()
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn traversal_runs_the_published_nodes() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 7, false, false) });
        assert!(unsafe { hirari_effect_chain_set_sample_rate(state, 48_000.0, 256) });
        let storage = unsafe { hirari_audio_buffer_storage_create() };
        assert_eq!(
            unsafe { hirari_audio_buffer_storage_reserve_buffer(storage, 2, 256) },
            0
        );
        assert!(unsafe { hirari_audio_buffer_storage_resize(storage, 2, 256) });
        let planes =
            unsafe { hirari_audio_buffer_storage_channel_pointers(storage as *const _) };
        for plane in 0..2 {
            let pointer = unsafe { *planes.add(plane) };
            assert!(!pointer.is_null());
            for frame in 0..256 {
                unsafe { pointer.add(frame).write(0.25) };
            }
        }
        let midi_state = unsafe { crate::midi_buffer_ops::hirari_midi_buffer_create() };
        unsafe { hirari_effect_chain_process(state, storage, midi_state, 120.0) };
        for plane in 0..2 {
            for frame in 0..256 {
                let sample = unsafe { (*planes.add(plane)).add(frame).read() };
                assert!(sample.is_finite(), "chain output went non-finite: {sample}");
            }
        }
        unsafe {
            crate::midi_buffer_ops::hirari_midi_buffer_destroy(midi_state);
            hirari_audio_buffer_storage_destroy(storage);
            hirari_effect_chain_destroy(state);
        }
    }

    #[test]
    fn clear_retires_nodes_and_metrics_stay_readable() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 0, false, false) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 0) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_metric(state as *const _, 0) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_metric(state as *const _, 1) },
            0
        );
        assert_eq!(unsafe { hirari_effect_chain_metric(state as *const _, 9) }, 0);
        let mut value = 0_u64;
        assert!(unsafe { hirari_effect_chain_metric_try(state as *const _, 2, &mut value) });
        unsafe { hirari_effect_chain_reclaim(state) };
        unsafe { clear(state) };
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 0);
        assert_eq!(
            unsafe { hirari_effect_chain_total_latency_samples(state as *const _) },
            0
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn diagnostics_never_block_on_control_thread_mutation() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        let address = state as usize;
        let stop = Arc::new(AtomicU8::new(0));
        let hog_stop = stop.clone();
        let hog = std::thread::spawn(move || {
            let state = address as *mut c_void;
            while hog_stop.load(Ordering::Acquire) == 0 {
                unsafe { hirari_effect_chain_set_sample_rate(state, 48_000.0, 512) };
            }
        });
        let mut worst = Duration::ZERO;
        for _ in 0..300 {
            let started = Instant::now();
            let mut value = 0_u64;
            unsafe {
                let _ = hirari_effect_chain_metric_try(state as *const _, 2, &mut value);
                let _ = hirari_effect_chain_metric_try(state as *const _, 1, &mut value);
            }
            worst = worst.max(started.elapsed());
            std::thread::yield_now();
        }
        stop.store(1, Ordering::Release);
        hog.join();
        assert!(
            worst < Duration::from_millis(5),
            "diagnostic reader blocked on the control thread: {worst:?}"
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn unknown_sandbox_paths_are_rejected_with_a_stable_text() {
        let state = chain();
        let path = b"/nonexistent/definitely-not-a-plugin.vst3";
        assert!(!unsafe {
            hirari_effect_chain_add_sandbox(state, path.as_ptr(), path.len())
        });
        let text =
            unsafe { hirari_effect_chain_last_sandbox_failure_text(state as *const _) };
        assert!(!text.is_null());
        let actual = unsafe { CStr::from_ptr(text) }.to_str().unwrap().to_string();
        assert!(!actual.is_empty());
        assert_eq!(
            unsafe { hirari_effect_chain_count(state as *const _) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_sandbox_state_error(state as *const _, 0) },
            STATE_ERROR_UNAVAILABLE
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }
}
