//! FFI surface for engine block config/marker/scale state, split out.
use super::*;

pub unsafe extern "C" fn hirari_engine_block_set_config_transition(
    state: *mut c_void,
    transition: u8,
) {
    if let Some(state) = block(state) {
        state.config_transition.store(transition != 0, Ordering::Release);
    }
}

/// Reports whether a callback observed a saturated sample position.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_range_overflowed(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.range_overflow.load(Ordering::Acquire))
    })
}

/// Empties the callback's own event collection, as a device re-preparation does.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_clear_midi_events(state: *mut c_void) {
    if let Some(state) = block(state) {
        unsafe { hirari_midi_buffer_clear(state.block_midi) };
    }
}

/// Binds an engine's block. Every handle must outlive the returned state.
///
/// # Safety
/// `handles` are read once; they belong to this one engine.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_create(
    handles: HirariEngineBlockHandles,
) -> *mut c_void {
    // The walk writes these from the audio callback only, so they are bound once
    // here instead of rebuilt for every block.
    let mut telemetry: Box<[HirariEngineTelemetry]> =
        vec![HirariEngineTelemetry::default(); 2].into();
    let telemetry_ptr = telemetry.as_mut_ptr();
    let mut ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let ids_ptr = ids.as_mut_ptr();
    let mut lefts: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let lefts_ptr = lefts.as_mut_ptr();
    let mut rights: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let rights_ptr = rights.as_mut_ptr();
    let click: Box<[[f32; MAX_BLOCK_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BLOCK_FRAMES], [0.0; MAX_BLOCK_FRAMES]]);
    let click_ptr = Box::into_raw(click);
    let send_pdc: Box<[[f32; MAX_BUS_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BUS_FRAMES], [0.0; MAX_BUS_FRAMES]]);
    let send_pdc_ptr = Box::into_raw(send_pdc);
    let seed = RoutingTrackDestination {
        id: 0,
        can_process: 0,
        is_bus: 0,
        channel_count: 0,
        work_channels: ptr::null(),
        bus_context: ptr::null_mut(),
    };
    let mut destinations: Box<[RoutingTrackDestination]> = vec![seed; MAX_TRACKS].into();
    let destinations_ptr = destinations.as_mut_ptr();
    let mut graph_ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let graph_ids_ptr = graph_ids.as_mut_ptr();
    let mut process_order: Box<[u32]> = vec![0u32; MAX_GRAPH_NODES].into();
    let process_order_ptr = process_order.as_mut_ptr();
    let fallback_notes: Box<[[u32; FALLBACK_PITCHES]; FALLBACK_CHANNELS]> =
        Box::new([[0u32; FALLBACK_PITCHES]; FALLBACK_CHANNELS]);
    let fallback_notes_ptr = Box::into_raw(fallback_notes);
    std::mem::forget(telemetry);
    std::mem::forget(ids);
    std::mem::forget(lefts);
    std::mem::forget(rights);
    std::mem::forget(destinations);
    std::mem::forget(graph_ids);
    std::mem::forget(process_order);
    Box::into_raw(Box::new(EngineBlockState {
        routing: handles.routing,
        track_table: handles.track_table,
        bounce: handles.bounce,
        input_blocks: handles.input_blocks,
        control_room: handles.control_room,
        mixer_telemetry: handles.mixer_telemetry,
        metronome: handles.metronome,
        master_limiter: handles.master_limiter,
        meter: handles.meter,
        tempo: handles.tempo,
        bus_system: handles.bus_system,
        sidechain: handles.sidechain,
        rings: handles.rings,
        midi_clock: handles.midi_clock,
        midi_snapshots: handles.midi_snapshots,
        preview_synth: handles.preview_synth,
        preview_sampler: handles.preview_sampler,
        mpe: handles.mpe,
        macro_control: handles.macro_control,
        layout_revision: handles.layout_revision,
        master_gain: AtomicU32::new(1.0f32.to_bits()),
        auto_record_mode: AtomicU32::new(0),
        shutdown: AtomicBool::new(false),
        callbacks_in_flight: AtomicU32::new(0),
        control_revision: AtomicU64::new(0),
        root_note: AtomicI32::new(0),
        scale_type: AtomicI32::new(0),
        playhead: AtomicU64::new(0),
        playing: AtomicBool::new(false),
        loop_enabled: AtomicBool::new(false),
        cycle_start: AtomicU64::new(0),
        cycle_end: AtomicU64::new(0),
        test_tone: AtomicBool::new(false),
        generation: AtomicU64::new(1),
        device_sample_rate: AtomicU64::new(44100f64.to_bits()),
        device_block_frames: AtomicU32::new(512),
        config_transition: AtomicBool::new(false),
        range_overflow: AtomicBool::new(false),
        preview_voices_need_reset: AtomicBool::new(false),
        monitor_offset: AtomicU32::new(0),
        talkback_offset: AtomicU32::new(0),
        block_midi: hirari_midi_buffer_create(),
        fallback_notes: fallback_notes_ptr,
        telemetry: telemetry_ptr,
        telemetry_active: AtomicU32::new(0),
        telemetry_lock: AtomicBool::new(false),
        telemetry_ids: ids_ptr,
        telemetry_left: lefts_ptr,
        telemetry_right: rights_ptr,
        click: click_ptr,
        send_pdc: send_pdc_ptr,
        destinations: destinations_ptr.cast(),
        graph_ids: graph_ids_ptr.cast(),
        process_order: process_order_ptr.cast(),
    }))
    .cast()
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { Box::from_raw(state.cast::<EngineBlockState>()) };
    unsafe {
        hirari_midi_buffer_destroy(state.block_midi);
        drop(Box::from_raw(state.fallback_notes));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry,
            2,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_left,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_right,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(state.click));
        drop(Box::from_raw(state.send_pdc));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.destinations,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.graph_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.process_order,
            MAX_GRAPH_NODES,
        )));
    }
}

/// The buffer index a control reader should copy.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub extern "C" fn hirari_engine_telemetry_active_index(state: *const c_void) -> u32 {
    block(state).map_or(0, |state| state.telemetry_active.load(Ordering::Acquire))
}

/// Copies one published metering block, or reports that the audio callback
/// holds it.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`] and
/// `out` writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_telemetry_snapshot(
    state: *const c_void,
    index: u32,
    out: *mut HirariEngineTelemetry,
) -> bool {
    let Some(state) = block(state) else {
        return false;
    };
    if index >= 2 || out.is_null() {
        return false;
    }
    if state.telemetry_lock.swap(true, Ordering::Acquire) {
        return false;
    }
    unsafe { *out = *state.telemetry.offset(index as isize) };
    state.telemetry_lock.store(false, Ordering::Release);
    true
}
