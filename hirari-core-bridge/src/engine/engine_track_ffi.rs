//! Track FFI exports, split out of engine_track.rs.
use super::*;

/// Builds a track and hands back its owning handle. `out_track` receives the
/// object pointer every other entry point addresses it with; it stays valid for
/// as long as one owner handle exists.
///
/// # Safety
/// `name` must be readable for `name_len` bytes, `revision` must be null or a
/// live [`hirari_layout_revision_create`] handle and `out_track` must be
/// writable or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_new(
    id: u32,
    name: *const u8,
    name_len: usize,
    kind: u32,
    revision: *mut c_void,
    out_track: *mut *mut c_void,
) -> *mut c_void {
    let bytes = if name.is_null() {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(name, name_len) }
    };
    let mut arc = Arc::new(Track::new(id, kind, revision_of(revision).cloned()));
    // Binding has to happen inside the allocation: the address it registers is
    // the one the track keeps until its last handle is released.
    if let Some(inner) = Arc::get_mut(&mut arc) {
        inner.bind(bytes);
    }
    if !out_track.is_null() {
        unsafe { *out_track = Arc::as_ptr(&arc).cast_mut().cast() };
    }
    Box::into_raw(Box::new(arc)).cast()
}

/// Clones an owning handle so a second holder can outlive the first.
///
/// # Safety
/// `owner` is null or a handle from [`hirari_track_new`] or
/// [`hirari_track_owner_retain`].
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_retain(owner: *mut c_void) -> *mut c_void {
    let Some(arc) = (unsafe { owner.cast::<Arc<Track>>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(arc.clone())).cast()
}

/// Drops one owning handle; the track's storages are destroyed with its last
/// one, in the order [`Track::drop`] keeps.
///
/// # Safety
/// `owner` is null or a live owning handle, released at most once.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_release(owner: *mut c_void) {
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner.cast::<Arc<Track>>()) });
    }
}

/// The object pointer inside an owning handle, or null.
///
/// # Safety
/// `owner` is null or a live owning handle.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_object(owner: *mut c_void) -> *mut c_void {
    let Some(arc) = (unsafe { owner.cast::<Arc<Track>>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    Arc::as_ptr(arc).cast_mut().cast()
}

/// # Safety
/// `track` is null or an object pointer from [`hirari_track_new`].
#[no_mangle]
pub unsafe extern "C" fn hirari_track_id(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| track.id)
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_type(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| track.kind)
}

/// The same answer as [`hirari_track_type`] in the byte the engine block
/// stores alongside each track.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_type_byte(track: *mut c_void) -> u8 {
    unsafe { hirari_track_type(track) as u8 }
}

/// Binds a bus track to the bus state its engine's bus system hands out. The
/// bus system is only mutated on the control thread, so resolution happens
/// after the replacement graph is committed, never while it is provisional. An
/// unresolved bus loses its slot so the graph cannot route to a stale one.
///
/// # Safety
/// `track` is null or a live track object and `bus_state` is a live bus or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_resolve_bus(
    track: *mut c_void,
    bus_state: *mut c_void,
    is_bus: bool,
) {
    let Some(inner) = (unsafe { (track as *mut Track).as_mut() }) else {
        return;
    };
    inner.bus_state = bus_state;
    inner.bus_id = if is_bus { inner.id } else { NO_BUS_ID };
}

/// One of the track's own storages, or null when it has none of that kind.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_storage(track: *mut c_void, kind: u32) -> *mut c_void {
    let Some(track) = track_ref(track) else {
        return std::ptr::null_mut();
    };
    match kind {
        STORAGE_STRIP => track.strip,
        STORAGE_CHAIN => track.chain,
        STORAGE_AUTOMATION => track.automation_store,
        STORAGE_FROZEN => track.frozen_store,
        STORAGE_RUNTIME => track.runtime,
        STORAGE_ARTICULATION => track.articulation,
        STORAGE_NAME => track.name_store,
        STORAGE_WORK_BUFFER => track.work,
        STORAGE_PLUGIN_MIDI => track.plugin_midi,
        STORAGE_BUS_STATE => track.bus_state,
        STORAGE_SEND_PRE => track.send_pre,
        STORAGE_SEND_POST => track.send_post,
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_mark_layout(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        track.mark_layout();
    }
}

/// Drops the frozen render so a stale one cannot survive a region, parameter or
/// automation mutation, and marks the layout dirty. The audio thread only ever
/// observes the raw pointer the freeze host published, and the store keeps the
/// replaced render until the control side retires it after quiescing.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_begin_edit(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_end_edit(track: *mut c_void) {
    unsafe { hirari_track_mark_layout(track) };
}

/// Rebuilds a track for a new device format. The status is the runtime's own
/// allocation report: 0 ready, 1 a growth the realtime guard declined, 2 an
/// impossible size and 3 an exhausted allocation — the last two are what the
/// host has to report, so a refused growth leaves the previous size in place.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_prepare(
    track: *mut c_void,
    sample_rate: f64,
    block_frames: u32,
) -> u8 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    unsafe { hirari_track_runtime_prepare(track.runtime, track.object(), sample_rate, block_frames) }
}

/// Whether the track's work block can carry a block of this length. The graph
/// skips a track that was prepared for a smaller one instead of rendering past
/// its buffers.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_can_process(track: *mut c_void, frames: u32) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_audio_buffer_storage_samples(track.work) >= frames
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_reset_for_offline_render(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_track_runtime_reset_for_offline_render(track.runtime) };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_latency_samples(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| unsafe {
        hirari_effect_chain_total_latency_samples(track.chain)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_requested(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, Track::pdc_requested)
}

/// User timing offset is intentionally separate from plugin PDC. Both values
/// share one preallocated transition line, but a graph rebuild must never erase
/// the musician's manual delay.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_delay_samples(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, Track::manual_delay)
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_delay_samples(track: *mut c_void, samples: u32) {
    let Some(track) = track_ref(track) else {
        return;
    };
    let previous = track.manual_delay();
    track.set_delays(track.pdc_requested(), samples);
    if track.manual_delay() != previous {
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_get_volume(track: *mut c_void) -> f32 {
    track_ref(track).map_or(0.0, |track| unsafe {
        hirari_channel_strip_gain(track.strip)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_volume(track: *mut c_void, value: f32) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_gain(track.strip, value) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_pan(track: *mut c_void, value: f32) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_pan(track.strip, value) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_solo(track: *mut c_void, solo: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_solo(track.strip, solo) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_muted(track: *mut c_void, muted: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_track_runtime_set_muted(track.runtime, u8::from(muted)) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_is_muted(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe { hirari_track_runtime_is_muted(track.runtime) })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_phase_inverted(track: *mut c_void, inverted: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_track_runtime_set_phase_invert(track.runtime, u8::from(inverted)) };
        track.mark_layout();
    }
}

/// Record arm reports whether it actually changed, because the automation
/// recorder admission and the undo entry both key off the transition.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_record_armed(track: *mut c_void, armed: bool) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    if unsafe { hirari_channel_strip_set_record_armed(track.strip, armed) } {
        track.mark_layout();
        return true;
    }
    false
}

/// Spatial coordinates are control-thread input, so a non-finite one is folded
/// to the centre instead of being published to the realtime panner.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_spatial(track: *mut c_void, x: f32, y: f32, z: f32) {
    let Some(track) = track_ref(track) else {
        return;
    };
    let sanitize = |value: f32| {
        if value.is_finite() {
            value.clamp(-100.0, 100.0)
        } else {
            0.0
        }
    };
    track.begin_edit();
    unsafe { hirari_track_runtime_set_spatial(track.runtime, sanitize(x), sanitize(y), sanitize(z)) };
    track.mark_layout();
}

/// # Safety
/// `track` is null or a live track object and `left`/`right` each hold `taps`
/// samples when they are not null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_hrtf_kernel(
    track: *mut c_void,
    left: *const f32,
    right: *const f32,
    taps: u32,
) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    if left.is_null() || right.is_null() || taps == 0 || taps > MAX_HRTF_TAPS {
        return false;
    }
    track.begin_edit();
    let changed = unsafe { hirari_track_holographic_panner_set_kernel(track.panner, left, right, taps) };
    if changed {
        track.mark_layout();
    }
    changed
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_clear_hrtf_kernel(track: *mut c_void) {
    let Some(track) = track_ref(track) else {
        return;
    };
    track.begin_edit();
    unsafe { hirari_track_holographic_panner_clear_kernel(track.panner) };
    track.mark_layout();
}

/// The audio-thread producer for write automation. The queue is deliberately
/// bounded and SPSC: recording a control-surface gesture must never grow a
/// vector, take an edit lock or allocate inside the callback. Rust gates the
/// write on the recorder mode and the punch range, so the callback only
/// publishes the current fader values of an armed track.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_record_state(track: *mut c_void, playhead: u64) {
    let Some(track) = track_ref(track) else {
        return;
    };
    if !unsafe { hirari_channel_strip_is_record_armed(track.strip) } {
        return;
    }
    let recorded = unsafe {
        hirari_track_automation_store_record(
            track.automation_store,
            hirari_automation_recorder_instance(),
            track.id,
            playhead,
            hirari_channel_strip_gain(track.strip),
            hirari_channel_strip_pan(track.strip),
        )
    };
    if recorded {
        if let Some(revision) = &track.revision {
            revision.realtime_automation_dirty.store(true, Ordering::Release);
        }
    }
}

/// The control-thread consumer: Rust drains the callback queue and appends the
/// captured points to the published volume and pan lists.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_flush_recorded_automation(track: *mut c_void) -> u32 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    let drained = unsafe { hirari_track_automation_store_flush_recorded(track.automation_store) };
    if drained != 0 {
        track.mark_layout();
    }
    drained
}

/// Hands a rendered, immutable region table to the track's realtime runtime.
/// The region store retires the previous generation late, so the pointer stays
/// readable until the next publication. This is the store's own callback rather
/// than a native entry point: the handle it receives is the track object.
pub unsafe extern "C" fn install_regions(
    track: *mut c_void,
    inputs: *const HirariTrackRegionRenderInput,
    length: usize,
    published: u8,
) {
    if let Some(track) = track_ref(track) {
        unsafe {
            hirari_track_runtime_publish_regions(track.runtime, inputs.cast(), length, published)
        };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_begin(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_channel_strip_begin_pitch_analysis(track.strip)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_finish(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_channel_strip_finish_pitch_analysis(track.strip) };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_running(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_channel_strip_is_pitch_analysis_running(track.strip)
    })
}

/// Installs one expression map, then marks the layout so the editor and the
/// playback path agree on which map is live.
///
/// # Safety
/// `track` is null or a live track object and `data` holds `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_expression_map_set(
    track: *mut c_void,
    kind: u8,
    data: *const u8,
    length: usize,
) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    let set =
        unsafe { hirari_track_midi_expression_map_set(track.articulation, kind, data, length) };
    track.mark_layout();
    set
}

/// Reclaims the retired plug-in nodes a chain kept for a callback in flight.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_reclaim_chain(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_effect_chain_reclaim(track.chain) };
    }
}

/// One sandbox slot's health, in the layout the maintenance pass reports.
fn sandbox_status(track: &Track, slot: usize) -> Option<HirariPluginSandboxStatus> {
    let mut raw = SandboxStatusOut::default();
    if !unsafe { hirari_effect_chain_sandbox_status(track.chain, slot, &mut raw) } {
        return None;
    }
    Some(HirariPluginSandboxStatus {
        plugin_index: raw.plugin_index,
        alive: raw.alive != 0,
        can_retry: raw.can_retry != 0,
        failure: raw.failure,
        dropped_output_midi: raw.dropped_output_midi,
        mailbox_overruns: raw.mailbox_overruns,
        input_midi_truncations: raw.input_midi_truncations,
        recovery_mode: u32::from(raw.recovery_mode),
    })
}

fn visit_statuses(track: &Track, context: *mut c_void, visit: Option<StatusVisitorFn>) -> bool {
    let Some(visit) = visit else { return false };
    for slot in 0..unsafe { hirari_effect_chain_sandbox_slot_count(track.chain) } {
        if let Some(status) = sandbox_status(track, slot) {
            if !unsafe { visit(context, track.id, &status) } {
                return false;
            }
        }
    }
    true
}

/// Reports every sandbox slot of one track. The maintenance pass walks the
/// graph under the tracks lock and stops on the first visitor refusal.
///
/// # Safety
/// `track` is null or a live track object; `visit` receives a status that is
/// only valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_visit_sandbox_statuses(
    track: *mut c_void,
    context: *mut c_void,
    visit: Option<StatusVisitorFn>,
) -> bool {
    track_ref(track).is_some_and(|track| visit_statuses(track, context, visit))
}

/// Restarts one track's rejected sandboxes and reports the resulting statuses.
/// A restart can block on a worker process, so the caller holds the track by
/// owning handle and has already released the topology lock.
///
/// # Safety
/// `track` is null or a live track object that outlives this call.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_maintain_sandboxes(
    track: *mut c_void,
    auto_restart: bool,
    context: *mut c_void,
    visit: Option<StatusVisitorFn>,
) -> u32 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    let recovered = unsafe { hirari_effect_chain_sandbox_maintain(track.chain, auto_restart) };
    if recovered != 0 {
        track.mark_layout();
    }
    visit_statuses(track, context, visit);
    recovered
}
