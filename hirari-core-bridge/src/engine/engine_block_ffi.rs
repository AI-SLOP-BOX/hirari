//! FFI surface for the engine block, split out of engine_block_host.rs.
use super::*;

pub unsafe extern "C" fn hirari_engine_block_begin(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    frames_out: *mut u32,
) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    let sample_rate = state.sample_rate();
    let (planes, channel_count, capacity) = track_runtime::channels_of(output);
    let capacity = capacity as usize;
    let off = off as usize;
    let frames = frames as usize;
    // The head silences the range the caller asked for, clamped to what the
    // buffer actually holds, before it reports the block as handled.
    let in_range = channel_count >= 2 && off < capacity;
    let prefix = frames.min(capacity.saturating_sub(off));
    if frames == 0 || !in_range {
        if frames != 0 && in_range {
            EngineBlockState::clear_window(planes, channel_count, off, prefix);
        }
        return 0;
    }
    if frames > MAX_BLOCK_FRAMES {
        EngineBlockState::clear_window(planes, channel_count, off, prefix);
        return 0;
    }
    let frames = prefix as u32;
    let offline_thread = hirari_bounce_offline_thread() != 0;
    if state.config_transition.load(Ordering::Acquire) && !offline_thread {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if hirari_bounce_offline_blocks(state.bounce, hirari_callback_thread_token()) != 0 {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if state.shutdown.load(Ordering::Acquire) {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    // Hot-swap quiesce: never sleep or spin here. The next block sees the
    // request again while the structural transaction is still active.
    if hirari_hot_swap_default_quiesce_requested() {
        hirari_hot_swap_default_acknowledge();
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    hirari_forensic_scratchpad_reset();
    let mut playhead = state.playhead.load(Ordering::Relaxed);
    let cycle_start = state.cycle_start.load(Ordering::Acquire);
    let cycle_end = state.cycle_end.load(Ordering::Acquire);
    if state.loop_enabled.load(Ordering::Acquire) && cycle_end > cycle_start && playhead >= cycle_end
    {
        playhead = cycle_start;
        state.playhead.store(cycle_start, Ordering::Relaxed);
    }
    // The per-chunk read offset travels with the window this callback renders:
    // the live driver input is addressed at the same offset as the master.
    state.monitor_offset.store(off as u32, Ordering::Relaxed);
    state.talkback_offset.store(off as u32, Ordering::Relaxed);
    let view = state.view();
    // Structural publication is double-buffered. Never process a state prepared
    // for a different device format; the next publication carries this one.
    if view.generation != state.generation.load(Ordering::Acquire) && !offline_thread {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if state
        .preview_voices_need_reset
        .swap(false, Ordering::AcqRel)
    {
        state.reset_preview_voices(view);
    }
    // A track's MIDI input is its own buffer: it only ever holds the events
    // addressed to it during this block, so the walk cannot replay the last.
    for &track in view.active() {
        if !track.is_null() {
            state.clear_midi(track);
        }
    }
    state.drain_commands(view.generation, view);
    // Keep the device callback primed while stopped, but do not traverse the
    // full graph unless a preview voice, a panic or an offline render owns the
    // path. Commands drain first so fader, pan, mute and solo edits stay
    // visible immediately while stopped.
    let offline_engaged =
        hirari_bounce_offline_engaged(state.bounce) != 0;
    let playing = state.playing.load(Ordering::Acquire);
    if !offline_engaged
        && !playing
        && !state.test_tone.load(Ordering::Relaxed)
        && !hirari_preview_sampler_is_active(state.preview_sampler)
        && !hirari_midi_playback_panic_pending(state.midi_clock)
    {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
    unsafe { hirari_midi_buffer_clear(state.block_midi) };
    if playhead > u64::MAX - u64::from(frames) {
        // A saturated sample position is reported, never wrapped around.
        state.range_overflow.store(true, Ordering::Release);
        return 0;
    }
    state.stage_midi(
        frames,
        playhead,
        sample_rate.max(1.0),
        offline_engaged || playing,
        view,
    );
    let Some((left, right, _, _)) = state.window(output, off as u32, frames) else {
        return 0;
    };
    state.render_preview(left, right, frames, playhead, sample_rate);
    if !frames_out.is_null() {
        unsafe { *frames_out = frames };
    }
    1
}

/// Chooses the next engine chunk for a hardware callback and rewinds the
/// playhead when it sits on the cycle end. The low 32 bits of the result hold
/// the frames to process; [`CHUNK_RESTART_PLAYHEAD`] means the caller retries
/// after the rewind this function already applied.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_chunk_plan(state: *mut c_void, remaining: u32) -> u64 {
    let Some(state) = block(state) else {
        return u64::from(remaining);
    };
    let plan = hirari_engine_chunk_plan(
        remaining,
        state.playing.load(Ordering::Acquire),
        state.loop_enabled.load(Ordering::Acquire),
        state.cycle_start.load(Ordering::Acquire),
        state.cycle_end.load(Ordering::Acquire),
        state.playhead.load(Ordering::Relaxed),
    );
    if plan & CHUNK_RESTART_PLAYHEAD != 0 {
        state
            .playhead
            .store(state.cycle_start.load(Ordering::Acquire), Ordering::Relaxed);
    }
    plan
}

/// Runs the post-graph tail of one engine block and advances the playhead the
/// head resolved.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`] and `output` is that
/// engine's block storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_finish(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
) {
    // The click scratch holds one kMaxAudioBlockSize block; the engine's head
    // gate clamps the block length before it reaches the tail.
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((left, right, planes, channel_count)) = state.window(output, off, frames) else {
        return;
    };
    let sample_rate = state.sample_rate();
    let master_gain = f32::from_bits(state.master_gain.load(Ordering::Acquire));
    let auto_record = state.auto_record_mode.load(Ordering::Relaxed);
    let view = state.view();
    let playhead = state.playhead.load(Ordering::Relaxed);
    let playing = state.playing.load(Ordering::Acquire);
    let talkback_offset = state.talkback_offset.load(Ordering::Relaxed);
    unsafe {
        state.capture_feedback(frames, view);
        state.publish_peaks(frames, view);
        let click = state.render_metronome(frames, playhead, sample_rate, playing);
        hirari_master_output_process(
            left,
            right,
            state.click(0),
            state.click(1),
            frames,
            master_gain,
            click,
        );
        if !state.master_limiter.is_null() {
            hirari_master_limiter_process(state.master_limiter, left, right, frames as usize, true);
        }
        hirari_master_meter_process(
            state.meter,
            left as *const f32,
            right as *const f32,
            frames as usize,
        );
        state.render_control_room(planes, channel_count, off, frames, talkback_offset, view);
        if playing && auto_record > 0 {
            for &track in view.active() {
                if !track.is_null() {
                    state.record(track, playhead);
                }
            }
        }
    }
    if !playing {
        return;
    }
    if playhead > u64::MAX - u64::from(frames) {
        state.range_overflow.store(true, Ordering::Release);
    } else {
        state.playhead.fetch_add(u64::from(frames), Ordering::Relaxed);
    }
}

/// Runs the graph half of one engine block: staging, feedback injection, the
/// live driver input, the routing destination table and the process-order walk
/// that fills the master window and every bus stage.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`] and `output` is that
/// engine's block storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_route(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
) {
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((master_left, master_right, _, _)) = state.window(output, off, frames) else {
        return;
    };
    let sample_rate = state.sample_rate();
    let monitor_offset = state.monitor_offset.load(Ordering::Relaxed);
    let generation = state.generation.load(Ordering::Acquire);
    let playing = state.playing.load(Ordering::Acquire);
    let playhead = state.playhead.load(Ordering::Relaxed);
    let view = state.view();
    let gating = Gating {
        offline_target_active: hirari_bounce_target_active(state.bounce) != 0,
        offline_target_id: hirari_bounce_target_id(state.bounce),
        any_solo: view
            .active()
            .iter()
            .copied()
            .filter(|track| !track.is_null())
            .any(|track| state.solo(track)),
    };
    unsafe {
        hirari_bus_system_clear(state.bus_system.cast_const(), frames);
        state.stage_tracks(frames, view, playing);
        state.inject_feedback(frames, view);
        state.feed_monitor_input(frames, monitor_offset, view);
        let destination_count = state.publish_destinations(frames, view, &gating);
        let destinations = slice::from_raw_parts(state.destinations.cast(), destination_count);
        // The Send PDC delay histories are Rust-owned; this section keeps a
        // control-thread mutation from retiring them mid-walk.
        hirari_routing_engine_enter_send_pdc_read(state.routing);
        state.walk_graph(
            frames,
            playhead,
            sample_rate.max(1.0) as u32,
            generation,
            view,
            &gating,
            master_left,
            master_right,
            destinations,
        );
        hirari_routing_engine_leave_send_pdc_read(state.routing);
    }
}

/// The transport position the callback last advanced.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_playhead(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.playhead.load(Ordering::Relaxed))
}

/// Seeks the transport.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_playhead(state: *mut c_void, playhead: u64) {
    if let Some(state) = block(state) {
        state.playhead.store(playhead, Ordering::Relaxed);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_is_playing(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| u8::from(state.playing.load(Ordering::Acquire)))
}

/// Starts or stops the transport. Stopping asks the callback to release every
/// voice it holds and requests the playback panic, because the voice state is
/// owned by that thread and must not be raced from the control side.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_playing(state: *mut c_void, playing: u8) {
    let Some(state) = block(state) else {
        return;
    };
    let playing = playing != 0;
    state.playing.store(playing, Ordering::Release);
    if !playing {
        state.preview_voices_need_reset.store(true, Ordering::Release);
        hirari_midi_playback_request_panic(state.midi_clock);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_loop_enabled(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.loop_enabled.load(Ordering::Acquire))
    })
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_loop(state: *mut c_void, enabled: u8) {
    if let Some(state) = block(state) {
        state
            .loop_enabled
            .store(enabled != 0, Ordering::Release);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_cycle_start(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.cycle_start.load(Ordering::Acquire))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_cycle_end(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.cycle_end.load(Ordering::Acquire))
}

/// Adopts a cycle, or refuses an empty one. Returns zero when the range was
/// rejected and nothing changed.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_cycle(
    state: *mut c_void,
    start_sample: u64,
    end_sample: u64,
    enabled: u8,
) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    if start_sample >= end_sample {
        return 0;
    }
    state.cycle_start.store(start_sample, Ordering::Release);
    state.cycle_end.store(end_sample, Ordering::Release);
    state.loop_enabled.store(enabled != 0, Ordering::Release);
    1
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_test_tone(state: *mut c_void, enabled: u8) {
    if let Some(state) = block(state) {
        state.test_tone.store(enabled != 0, Ordering::Release);
    }
}

/// The device configuration generation the callbacks and queued commands are
/// tagged with.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_generation(state: *const c_void) -> u64 {
    block(state).map_or(1, |state| state.generation.load(Ordering::Acquire))
}

/// Moves to the next device configuration generation.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_bump_generation(state: *mut c_void) -> u64 {
    block(state).map_or(0, |state| {
        state.generation.fetch_add(1, Ordering::AcqRel)
    })
}

/// The sample rate the graph was last prepared for.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_device_sample_rate(state: *const c_void) -> f64 {
    block(state)
        .map_or(0.0, |state| f64::from_bits(state.device_sample_rate.load(Ordering::Acquire)))
}

/// The block size the graph was last prepared for.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_device_block_frames(state: *const c_void) -> u32 {
    block(state).map_or(0, |state| state.device_block_frames.load(Ordering::Acquire))
}

/// Records the device format the graph is being prepared for, before any track
/// re-preparation reads it back.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_device_format(
    state: *mut c_void,
    sample_rate: f64,
    block_frames: u32,
) {
    if let Some(state) = block(state) {
        state
            .device_sample_rate
            .store(sample_rate.to_bits(), Ordering::Release);
        state.device_block_frames.store(block_frames, Ordering::Release);
    }
}

/// The layout revision the engine's tracks mark.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_layout_revision(state: *const c_void) -> *mut c_void {
    block(state).map_or(ptr::null_mut(), |state| state.layout_revision)
}

/// The master fader the block tail mixes with.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_master_gain(state: *const c_void) -> f32 {
    block(state).map_or(1.0, |state| f32::from_bits(state.master_gain.load(Ordering::Acquire)))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_master_gain(state: *mut c_void, gain: f32) {
    if let Some(state) = block(state) {
        state.master_gain.store(gain.to_bits(), Ordering::Release);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_auto_record_mode(state: *mut c_void, mode: u32) {
    if let Some(state) = block(state) {
        state.auto_record_mode.store(mode, Ordering::Release);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_is_shutdown(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.shutdown.load(Ordering::Acquire))
    })
}

/// Sets the shutdown flag and reports whether it was already set, so a second
/// shutdown can skip the once-only work.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_take_shutdown(state: *mut c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.shutdown.swap(true, Ordering::AcqRel))
    })
}

/// Counts a device callback as it enters and leaves the graph.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_callback_enter(state: *mut c_void) {
    if let Some(state) = block(state) {
        state.callbacks_in_flight.fetch_add(1, Ordering::Acquire);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`] and a
/// matching [`hirari_engine_block_callback_enter`] must have run.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_callback_leave(state: *mut c_void) {
    if let Some(state) = block(state) {
        state.callbacks_in_flight.fetch_sub(1, Ordering::Release);
    }
}

/// Waits for every callback in flight to finish. A control/shutdown path, never
/// the realtime callback: yielding keeps it from spinning a whole core while it
/// avoids putting a mutex in the audio path.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_join_callbacks(state: *const c_void) {
    let Some(state) = block(state) else {
        return;
    };
    while state.callbacks_in_flight.load(Ordering::Acquire) != 0 {
        std::thread::yield_now();
    }
}

/// The control-plane edit token, mixed with the layout revision and the MIDI
/// snapshot revision into a project state revision.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_control_revision(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.control_revision.load(Ordering::Acquire))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_mark_control(state: *mut c_void) -> u64 {
    block(state).map_or(0, |state| {
        state.control_revision.fetch_add(1, Ordering::AcqRel)
    })
}

/// The project root note a save writes back into its document.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_root_note(state: *const c_void) -> i32 {
    block(state).map_or(0, |state| state.root_note.load(Ordering::Relaxed))
}

/// The project scale type a save writes back into its document.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_scale_type(state: *const c_void) -> i32 {
    block(state).map_or(0, |state| state.scale_type.load(Ordering::Relaxed))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_project_scale(
    state: *mut c_void,
    root: i32,
    scale: i32,
) {
    if let Some(state) = block(state) {
        state.root_note.store(root, Ordering::Relaxed);
        state.scale_type.store(scale, Ordering::Relaxed);
    }
}

/// The engine track table the block was created with. Rust analysis reads walk
/// its published snapshot directly instead of asking C++ for the graph.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_track_table(state: *const c_void) -> *mut c_void {
    block(state).map_or(ptr::null_mut(), |state| state.track_table)
}

/// The master limiter the block tail guards. Latency is the device block plus its
/// lookahead, so a read of the engine's delay needs no accessor of its own.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_master_limiter(state: *const c_void) -> *mut c_void {
    block(state).map_or(ptr::null_mut(), |state| state.master_limiter)
}

/// Prepares one track for the device format this block already holds — the step
/// a new bootstrap track and every session re-prepare needs, so neither reads a
/// rate and length off the engine.
///
/// Returns the runtime's allocation status: 0 ready, 1 a growth the realtime
/// guard declined, 2 an impossible size, 3 an exhausted allocation.
///
/// # Safety
/// `state` must be a pointer from [`hirari_engine_block_create`] and `track` a
/// live track object the caller keeps alive across the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_prepare_track(
    state: *mut c_void,
    track: *mut c_void,
) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    unsafe {
        hirari_track_prepare(
            track,
            state.sample_rate(),
            state.device_block_frames.load(Ordering::Relaxed),
        )
    }
}

/// Prepares every live track for a new device format. A freeze the new rate
/// cannot replay is dropped and retired, taking the layout revision with it, the
/// track's blocks and DSP retune, and a work block that reached the new length
/// re-offers its taps to the sidechain graph at this generation.
///
/// Returns 0 once the graph is prepared, or the first allocation status a block
/// growth could not satisfy — 2 an impossible size, 3 an exhausted allocation —
/// which stops the walk so the host can report a failed preparation.
///
/// # Safety
/// `state` must be a pointer from [`hirari_engine_block_create`] and the caller
/// must hold the engine's tracks lock, so the table is one graph for the walk.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_prepare_tracks(state: *mut c_void) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    let sample_rate = state.sample_rate();
    let block_frames = state.device_block_frames.load(Ordering::Relaxed);
    let rate = sample_rate as u32;
    let generation = state.generation.load(Ordering::Acquire);
    let count = unsafe { hirari_engine_track_count(state.track_table) };
    let pointers = unsafe { hirari_engine_track_ptrs(state.track_table) };
    for &track in unsafe { slice::from_raw_parts(pointers, count) } {
        if track.is_null() {
            continue;
        }
        let Some(runtime) = state.runtime(track) else {
            continue;
        };
        if runtime.invalidate_stale_freeze(rate) {
            unsafe { hirari_track_end_edit(track) };
        }
        let status = runtime.prepare(track, sample_rate, block_frames);
        if status != 0 {
            return status;
        }
        let work = runtime.work_storage();
        if unsafe { hirari_audio_buffer_storage_samples(work) } < block_frames {
            continue;
        }
        let left = unsafe { hirari_audio_buffer_storage_read_pointer(work, 0, 0) };
        let right = if unsafe { hirari_audio_buffer_storage_channels(work) } > 1 {
            unsafe { hirari_audio_buffer_storage_read_pointer(work, 1, 0) }
        } else {
            left
        };
        unsafe {
            hirari_sidechain_engine_refresh_source(
                state.sidechain,
                runtime.track_id(),
                left,
                right,
                block_frames,
                rate,
                generation,
            )
        };
    }
    0
}

/// Closes callback admission while the control thread replaces the buffers a
/// callback traverses.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[path = "engine_block_ffi_state.rs"]
mod block_ffi_state;
pub use block_ffi_state::*;
