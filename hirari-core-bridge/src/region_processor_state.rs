//! Rust-owned settings and channel traversal for the native region processor.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

struct RegionProcessorState {
    gain: AtomicU32,
    fade_in_samples: AtomicU32,
    fade_in_type: AtomicU8,
    fade_out_samples: AtomicU32,
    fade_out_type: AtomicU8,
}

impl RegionProcessorState {
    fn new() -> Self {
        Self {
            gain: AtomicU32::new(1.0_f32.to_bits()),
            fade_in_samples: AtomicU32::new(0),
            fade_in_type: AtomicU8::new(0),
            fade_out_samples: AtomicU32::new(0),
            fade_out_type: AtomicU8::new(0),
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_region_processor_create() -> *mut c_void {
    Box::into_raw(Box::new(RegionProcessorState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<RegionProcessorState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_set_gain(state: *mut c_void, gain: f32) {
    let Some(state) = (unsafe { state.cast::<RegionProcessorState>().as_ref() }) else {
        return;
    };
    let value = if gain.is_finite() { gain.clamp(0.0, 4.0) } else { 1.0 };
    state.gain.store(value.to_bits(), Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_set_fade_in(
    state: *mut c_void,
    samples: u32,
    fade_type: u8,
) {
    if let Some(state) = unsafe { state.cast::<RegionProcessorState>().as_ref() } {
        state.fade_in_samples.store(samples, Ordering::Release);
        state.fade_in_type.store(fade_type, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_set_fade_out(
    state: *mut c_void,
    samples: u32,
    fade_type: u8,
) {
    if let Some(state) = unsafe { state.cast::<RegionProcessorState>().as_ref() } {
        state.fade_out_samples.store(samples, Ordering::Release);
        state.fade_out_type.store(fade_type, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_process(
    state: *mut c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    samples: u32,
    offset: u32,
    size: u32,
    region_relative_pos: u64,
) {
    let Some(state) = (unsafe { state.cast::<RegionProcessorState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || offset >= samples || channel_count == 0 || size == 0 {
        return;
    }
    let count = size.min(samples - offset);
    let gain = f32::from_bits(state.gain.load(Ordering::Acquire));
    let fade_in_samples = state.fade_in_samples.load(Ordering::Acquire);
    let fade_type = state.fade_in_type.load(Ordering::Acquire);
    let channel_ptrs = unsafe { std::slice::from_raw_parts_mut(channels, channel_count as usize) };
    for channel in channel_ptrs {
        if channel.is_null() {
            continue;
        }
        let start = unsafe { channel.add(offset as usize) };
        unsafe {
            crate::region_processing::hirari_region_processor_apply_gain_fade(
                start, count, region_relative_pos, gain, fade_in_samples, fade_type,
            );
        }
    }
}
