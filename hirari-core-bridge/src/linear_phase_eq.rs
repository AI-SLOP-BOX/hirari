use crate::fft::FftPlan;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const FFT_SIZE: usize = 1024;
const MAX_CHANNELS: usize = 2;
const BLOCK_SIZE: usize = FFT_SIZE / 2;

struct LinearPhaseRuntime {
    fft: FftPlan,
    real: [f32; FFT_SIZE],
    imag: [f32; FFT_SIZE],
    overlap: [[f32; FFT_SIZE]; MAX_CHANNELS],
}

impl LinearPhaseRuntime {
    fn new() -> Option<Self> {
        Some(Self {
            fft: FftPlan::new(FFT_SIZE)?,
            real: [0.0; FFT_SIZE],
            imag: [0.0; FFT_SIZE],
            overlap: [[0.0; FFT_SIZE]; MAX_CHANNELS],
        })
    }

    fn reset(&mut self) {
        for channel in &mut self.overlap {
            channel.fill(0.0);
        }
        self.real.fill(0.0);
        self.imag.fill(0.0);
    }

    fn process_channel(&mut self, samples: &mut [f32], channel: usize, gains: [f32; 3]) {
        for block in samples.chunks_mut(BLOCK_SIZE) {
            let frames = block.len();
            self.real.fill(0.0);
            self.imag.fill(0.0);
            for (index, sample) in block.iter().enumerate() {
                self.real[index] = if sample.is_finite() {
                    sample.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
            }
            self.fft.forward(&mut self.real, &mut self.imag);
            for index in 0..FFT_SIZE {
                let bin = index.min(FFT_SIZE - index);
                let normalized_frequency = bin as f32 / FFT_SIZE as f32;
                let gain = if normalized_frequency < 0.1 {
                    gains[0]
                } else if normalized_frequency < 0.3 {
                    gains[1]
                } else {
                    gains[2]
                };
                self.real[index] *= gain;
                self.imag[index] *= gain;
            }
            self.fft.inverse(&mut self.real, &mut self.imag);
            for (index, sample) in block.iter_mut().enumerate() {
                let output = self.real[index] + self.overlap[channel][index];
                *sample = if output.is_finite() {
                    output.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
            }
            let remaining = FFT_SIZE - frames;
            for index in 0..remaining {
                let old_tail = self.overlap[channel][frames + index];
                let new_tail = self.real[frames + index];
                let overlap = old_tail + new_tail;
                self.overlap[channel][index] = if overlap.is_finite() { overlap } else { 0.0 };
            }
            self.overlap[channel][remaining..].fill(0.0);
        }
    }
}

struct LinearPhaseState {
    runtime: UnsafeCell<LinearPhaseRuntime>,
    gains: [AtomicU32; 3],
}

// The processing runtime has one serialized audio/lifecycle writer; gain
// controls are independently published atomically from the control thread.
unsafe impl Sync for LinearPhaseState {}

#[no_mangle]
pub extern "C" fn hirari_linear_phase_eq_create() -> *mut c_void {
    LinearPhaseRuntime::new().map_or(std::ptr::null_mut(), |runtime| {
        Box::into_raw(Box::new(LinearPhaseState {
            runtime: UnsafeCell::new(runtime),
            gains: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
        }))
        .cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_phase_eq_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<LinearPhaseState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_phase_eq_set_gains(
    state: *const c_void,
    low: f32,
    mid: f32,
    high: f32,
) {
    let Some(state) = (unsafe { state.cast::<LinearPhaseState>().as_ref() }) else {
        return;
    };
    for (slot, value) in state.gains.iter().zip([low, mid, high]) {
        let value = if value.is_finite() {
            value.clamp(0.0, 16.0)
        } else {
            1.0
        };
        slot.store(value.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_phase_eq_get_gain(state: *const c_void, band: u32) -> f32 {
    let Some(state) = (unsafe { state.cast::<LinearPhaseState>().as_ref() }) else {
        return 1.0;
    };
    state
        .gains
        .get(band as usize)
        .map_or(0.0, |gain| f32::from_bits(gain.load(Ordering::Relaxed)))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_phase_eq_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<LinearPhaseState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_phase_eq_process(
    state: *const c_void,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<LinearPhaseState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || frames == 0 {
        return;
    }
    let gains =
        std::array::from_fn(|index| f32::from_bits(state.gains[index].load(Ordering::Relaxed)));
    let runtime = unsafe { &mut *state.runtime.get() };
    for channel in 0..(channel_count as usize).min(MAX_CHANNELS) {
        let pointer = unsafe { *channels.add(channel) };
        if !pointer.is_null() {
            let samples = unsafe { std::slice::from_raw_parts_mut(pointer, frames as usize) };
            runtime.process_channel(samples, channel, gains);
        }
    }
}
