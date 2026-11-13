//! Effect-chain parameter/status FFI, split out of effect_chain_host.rs.
use super::*;

pub(crate) fn sandbox_parameter_name(chain: *const c_void, index: u32, parameter_id: u32) -> String {
    let Some(host) = (unsafe { host(chain.cast_mut()) }) else {
        return String::new();
    };
    entry_node(host, index)
        .map(|node| node.parameter_name(parameter_id))
        .unwrap_or_default()
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

/// The polled non-finite sample count. A contended read keeps the last
/// measurement, so a UI poll never waits behind a control-thread edit.
#[no_mangle]
pub unsafe extern "C" fn hirari_effect_chain_non_finite_samples(state: *const c_void) -> u64 {
    let Some(host) = (unsafe { host(state.cast_mut()) }) else {
        return 0;
    };
    let Ok(guard) = host.inner.try_lock() else {
        return host.last_non_finite.load(Ordering::Relaxed);
    };
    let value = node_metric(&guard, 1);
    host.last_non_finite.store(value, Ordering::Relaxed);
    value
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
    sidechain_gate: *mut c_void,
    sidechain_track: u32,
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
    chain.set_sidechain_context(context.sidechain_gate, context.sidechain_track, index);
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
    sidechain_gate: *mut c_void,
    sidechain_track: u32,
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
        sidechain_gate,
        sidechain_track,
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
#[path = "effect_chain_host_tests.rs"]
mod effect_chain_host_tests;
