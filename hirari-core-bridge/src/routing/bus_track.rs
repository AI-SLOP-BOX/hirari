//! Rust-owned realtime state and input-stage processing for bus tracks.

use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MAX_BUS_FRAMES: usize = 8192;

struct BusTrackState {
    input_gain: AtomicU32,
    invert_phase: AtomicBool,
    read_post_fx: AtomicBool,
}

impl BusTrackState {
    fn new() -> Self {
        Self {
            input_gain: AtomicU32::new(1.0_f32.to_bits()),
            invert_phase: AtomicBool::new(false),
            read_post_fx: AtomicBool::new(true),
        }
    }
}

unsafe fn state(handle: *const c_void) -> Option<&'static BusTrackState> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &*handle.cast::<BusTrackState>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_bus_track_create() -> *mut c_void {
    Box::into_raw(Box::new(BusTrackState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_track_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<BusTrackState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_track_set_input_gain(handle: *mut c_void, gain: f32) {
    let Some(state) = (unsafe { state(handle) }) else {
        return;
    };
    let gain = if gain.is_finite() {
        gain.clamp(0.0, 4.0)
    } else {
        1.0
    };
    state.input_gain.store(gain.to_bits(), Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_track_set_phase_inverted(handle: *mut c_void, inverted: bool) {
    if let Some(state) = unsafe { state(handle) } {
        state.invert_phase.store(inverted, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_track_set_read_post_fx(handle: *mut c_void, post_fx: bool) {
    if let Some(state) = unsafe { state(handle) } {
        state.read_post_fx.store(post_fx, Ordering::Release);
    }
}

/// Copies the selected bus stage into the destination and applies input gain
/// and polarity in place. No buffer allocation or locking occurs here.
#[no_mangle]
pub unsafe extern "C" fn hirari_bus_track_fetch_audio(
    track_handle: *const c_void,
    bus_handle: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) -> bool {
    let frames = frames as usize;
    if left.is_null() || right.is_null() || frames == 0 || frames > MAX_BUS_FRAMES {
        return false;
    }
    let left_samples = unsafe { slice::from_raw_parts_mut(left, frames) };
    let right_samples = unsafe { slice::from_raw_parts_mut(right, frames) };
    left_samples.fill(0.0);
    right_samples.fill(0.0);

    let Some(track) = (unsafe { state(track_handle) }) else {
        return false;
    };
    if !unsafe {
        crate::bus_audio::hirari_bus_audio_read(
            bus_handle,
            left,
            right,
            frames as u32,
            track.read_post_fx.load(Ordering::Acquire),
        )
    } {
        return false;
    }

    let gain = f32::from_bits(track.input_gain.load(Ordering::Acquire));
    let polarity = if track.invert_phase.load(Ordering::Acquire) {
        -1.0
    } else {
        1.0
    };
    let scale = gain * polarity;
    for sample in left_samples.iter_mut().chain(right_samples.iter_mut()) {
        *sample *= scale;
    }
    true
}
