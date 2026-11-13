//! FFI exports for the bounce driver, split out of bounce_driver.rs.
use super::*;

/// Renders one project bounce. Returns true only when a complete file was
/// published; every other path leaves a terminal state behind.
///
/// # Safety
/// `host` must be the engine pointer the operations expect, `session` the
/// engine's own bounce session, `ops` a complete table and `path` `path_len`
/// readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_driver(
    host: *mut c_void,
    session: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: *const u8,
    path_len: usize,
    format: u32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let Some(session) = bounce_session(session) else {
        return false;
    };
    // SAFETY: the caller guarantees a complete table for this call.
    let ops = unsafe { &*ops };
    if missing(ops) {
        return false;
    }
    let (reset, store, render, render_chunk) = (
        ops.reset.unwrap(),
        ops.store.unwrap(),
        ops.render.unwrap(),
        ops.render_chunk.unwrap(),
    );
    let (read_transport, set_transport) = (ops.read_transport.unwrap(), ops.set_transport.unwrap());

    if !hirari_bounce_output_path_supported(path, path_len, format) {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        log(1, b"BOUNCE_UNSUPPORTED_FORMAT_OR_PATH");
        return false;
    }

    match session.begin() {
        OWNERSHIP_ACQUIRED => {}
        OWNERSHIP_CANCELLED => {
            session.publish(0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        _ => return false,
    }

    let plan_result = match gather_plan(host, store, session) {
        Ok(plan) => plan,
        Err(reason) => {
            log(
                1,
                if reason == BOUNCE_PLAN_INVALID_RANGE {
                    b"BOUNCE_INVALID_RANGE"
                } else {
                    b"BOUNCE_INVALID_SAMPLE_RATE"
                },
            );
            session.publish(0.0, BOUNCE_STATE_FAILED);
            session.end();
            return false;
        }
    };

    // Public formats map onto the Rust stream ABI: PCM16, WAVE64 float,
    // float32 RIFF/RF64, PCM24.
    const STREAM_FORMATS: [u8; 4] = [1, 3, 0, 2];
    let stream = StreamGuard(unsafe {
        hirari_bounce_stream_create(
            path,
            path_len,
            plan_result.total_samples,
            plan_result.output_rate,
            STREAM_FORMATS[format as usize],
        )
    });
    if stream.0.is_null() {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        log(0, b"BOUNCE_STREAM_OPEN_FAILED");
        session.end();
        return false;
    }

    let mut original_playhead = 0_u64;
    let mut original_playing = 0_u8;
    // SAFETY: both out-pointers are live locals; the transport belongs to the
    // engine that owns this session.
    unsafe { read_transport(host, &mut original_playhead, &mut original_playing) };
    unsafe { set_transport(host, plan_result.playhead_start, 1) };
    unsafe { reset(host) };

    let chunk = unsafe { render_chunk(host) }.clamp(1, MAX_RENDER_CHUNK) as usize;
    let mut left = vec![0.0_f32; chunk];
    let mut right = vec![0.0_f32; chunk];
    let dither_depth = match format {
        0 => 256.0_f32,
        3 => 1.0_f32,
        _ => 0.0_f32,
    };
    let mut dither = (dither_depth != 0.0).then(Dither::new);
    let total = plan_result.total_samples.min(u64::from(u32::MAX));
    let mut progress = 0.0_f32;
    let mut offset = 0_u64;
    while offset < total {
        let bits = session.control_bits();
        if bits & CONTROL_CANCEL != 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            session.end();
            session.publish(0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        if bits & CONTROL_PAUSE != 0 {
            session.publish(progress, BOUNCE_STATE_PAUSED);
            std::thread::sleep(PAUSE_POLL);
            continue;
        }
        session.publish(progress, BOUNCE_STATE_RENDERING);
        let frames = (total - offset).min(chunk as u64) as u32;
        left[..frames as usize].fill(0.0);
        right[..frames as usize].fill(0.0);
        // SAFETY: the host renders into the `frames` entries of each buffer.
        unsafe { render(host, left.as_mut_ptr(), right.as_mut_ptr(), frames) };
        if let Some(dither) = dither.as_mut() {
            for sample in 0..frames as usize {
                let (noise_left, noise_right) = dither.next();
                left[sample] += noise_left * dither_depth;
                right[sample] += noise_right * dither_depth;
            }
        }
        let channels: [*const f32; 2] = [left.as_ptr(), right.as_ptr()];
        if unsafe { hirari_bounce_stream_write(stream.0, channels.as_ptr(), frames) } == 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            session.end();
            session.publish(0.0, BOUNCE_STATE_FAILED);
            log(0, b"BOUNCE_STREAM_WRITE_FAILED");
            return false;
        }
        offset += u64::from(frames);
        progress = offset as f32 / total as f32;
    }

    unsafe { set_transport(host, original_playhead, original_playing) };
    match session.finish_stream(stream.0) {
        1 => {
            session.end();
            session.publish(1.0, BOUNCE_STATE_COMPLETED);
            true
        }
        published => {
            session.end();
            session.publish(0.0, BOUNCE_STATE_FAILED);
            log(
                0,
                if published == 0 {
                    b"BOUNCE_CANCELLED"
                } else {
                    b"BOUNCE_STREAM_PUBLISH_FAILED"
                },
            );
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_session_create() -> *mut c_void {
    Box::into_raw(Box::new(BounceSession::new())).cast()
}

/// # Safety
/// `handle` must come from [`hirari_bounce_session_create`] and have no live
/// borrower. Callers join the worker before destroying the session.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_session_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle.cast::<BounceSession>()) });
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_thread() -> u8 {
    u8::from(offline_thread())
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_active(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.offline_active.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_engaged(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(BounceSession::engaged))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_blocks(handle: *mut c_void, token: u64) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.blocks_callback(token)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_target_active(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.target_active.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_target_id(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(0, |s| s.target_id.load(Ordering::Acquire))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_pre_fader(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.pre_fader.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_include_inserts(handle: *mut c_void) -> u8 {
    u8::from(
        bounce_session(handle)
            .map_or(true, |s| s.include_inserts.load(Ordering::Acquire)),
    )
}

#[no_mangle]
pub extern "C" fn hirari_bounce_tail_millis(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(0, |s| s.tail_millis.load(Ordering::Acquire))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_offline_target(handle: *mut c_void, track_id: u32) {
    if let Some(session) = bounce_session(handle) {
        session.target_id.store(track_id, Ordering::Release);
        session.target_active.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_clear_offline_target(handle: *mut c_void) {
    if let Some(session) = bounce_session(handle) {
        session.target_active.store(false, Ordering::Release);
        session.target_id.store(0, Ordering::Release);
        session.tail_millis.store(0, Ordering::Release);
        session.pre_fader.store(false, Ordering::Release);
        session.include_inserts.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_tail_seconds(handle: *mut c_void, seconds: f32) {
    if let Some(session) = bounce_session(handle) {
        // Bounds and non-finite rejection are enforced by the bridge.
        session
            .tail_millis
            .store((seconds * 1000.0) as u32, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_render_options(
    handle: *mut c_void,
    pre_fader: u8,
    include_inserts: u8,
) {
    if let Some(session) = bounce_session(handle) {
        session
            .pre_fader
            .store(pre_fader != 0, Ordering::Release);
        session
            .include_inserts
            .store(include_inserts != 0, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_progress(handle: *mut c_void) -> f32 {
    bounce_session(handle).map_or(0.0, |s| {
        f32::from_bits(s.progress.load(Ordering::Acquire)).clamp(0.0, 1.0)
    })
}

#[no_mangle]
pub extern "C" fn hirari_bounce_state(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(BOUNCE_STATE_IDLE, BounceSession::state_value)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_control_bits(handle: *mut c_void) -> u8 {
    bounce_session(handle).map_or(0, BounceSession::control_bits)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_cancel(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_cancel)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_pause(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_pause)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_resume(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_resume)
}

/// # Safety
/// `host`, `ops` and `path` follow the same contract as
/// [`hirari_bounce_render_driver`]; the engine must outlive the worker it
/// starts, which it does because teardown joins through [`hirari_bounce_join`].
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_start_async(
    handle: *mut c_void,
    host: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: *const u8,
    path_len: usize,
    format: u32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let Some(session) = bounce_session(handle) else {
        return false;
    };
    // SAFETY: the caller guarantees a complete table with static storage.
    let ops = unsafe { &*ops };
    if ops.run.is_none() {
        return false;
    }
    if format > 3 || path_len == 0 {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        return false;
    }
    // SAFETY: the caller passes `path_len` readable bytes for this call.
    let path = unsafe { slice::from_raw_parts(path, path_len) };
    session.start_async(host, ops, path, format)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_join(handle: *mut c_void) {
    if let Some(session) = bounce_session(handle) {
        session.join();
    }
}
