use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

struct StepFilterRuntime {
    smooth_cutoff: f32,
    filter_state: [f32; 2],
}

/// Realtime step filter state. Step values are atomic because the UI can edit
/// the pattern while the audio thread processes blocks.
pub struct StepFilterEngine {
    sample_rate_bits: AtomicU64,
    resonance_bits: AtomicU32,
    steps: [AtomicU32; 16],
    runtime: UnsafeCell<StepFilterRuntime>,
}

impl StepFilterEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate_bits: AtomicU64::new(sample_rate.to_bits()),
            resonance_bits: AtomicU32::new(0.1f32.to_bits()),
            steps: std::array::from_fn(|_| AtomicU32::new(0.5f32.to_bits())),
            runtime: UnsafeCell::new(StepFilterRuntime {
                smooth_cutoff: 0.5,
                filter_state: [0.0; 2],
            }),
        }
    }

    pub fn prepare(&self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 1000.0 {
            self.sample_rate_bits
                .store(sample_rate.to_bits(), Ordering::Release);
        }
        self.reset();
    }

    pub fn reset(&self) {
        // Lifecycle calls are serialized against processing by the host.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.filter_state = [0.0; 2];
        runtime.smooth_cutoff = f32::from_bits(self.steps[0].load(Ordering::Acquire));
    }

    pub fn set_step_value(&self, step: u32, value: f32) {
        if step < 16 {
            self.steps[step as usize].store(value.to_bits(), Ordering::Release);
        }
    }

    pub fn set_resonance(&self, value: f32) {
        self.resonance_bits
            .store(value.to_bits(), Ordering::Release);
    }

    pub unsafe fn process(
        &self,
        channels: *mut *mut f32,
        channel_count: u32,
        frames: u32,
        bpm: f64,
        context_sample_rate: f64,
        block_start: u64,
    ) {
        if channels.is_null() || frames == 0 || channel_count == 0 {
            return;
        }
        let channel_count = channel_count.min(2) as usize;
        let pointers = unsafe { std::slice::from_raw_parts(channels, channel_count) };
        if pointers.iter().any(|pointer| pointer.is_null()) {
            return;
        }
        let bpm = if bpm.is_finite() {
            bpm.clamp(20.0, 300.0)
        } else {
            120.0
        };
        let sample_rate = if context_sample_rate.is_finite() {
            context_sample_rate
        } else {
            0.0
        };
        let samples_per_step = (sample_rate * 60.0 / bpm / 4.0).max(1.0);
        let runtime = unsafe { &mut *self.runtime.get() };

        for frame in 0..frames as usize {
            let absolute = block_start.wrapping_add(frame as u64);
            let step = ((absolute as f64 / samples_per_step).floor() as u32) & 15;
            let raw_target = f32::from_bits(self.steps[step as usize].load(Ordering::Acquire));
            let target = if raw_target.is_finite() {
                raw_target
            } else {
                0.5
            }
            .clamp(0.001, 0.99);
            runtime.smooth_cutoff += (target - runtime.smooth_cutoff) * 0.02;
            let alpha = runtime.smooth_cutoff.clamp(0.001, 0.99);
            for (channel, pointer) in pointers.iter().enumerate() {
                let input = unsafe { *pointer.add(frame) };
                let input = if input.is_finite() { input } else { 0.0 };
                runtime.filter_state[channel] += alpha * (input - runtime.filter_state[channel]);
                let output = runtime.filter_state[channel];
                unsafe {
                    *pointer.add(frame) = if output.is_finite() { output } else { 0.0 };
                }
            }
        }
    }

    pub fn audit(&self) -> bool {
        let runtime = unsafe { &*self.runtime.get() };
        f64::from_bits(self.sample_rate_bits.load(Ordering::Acquire)).is_finite()
            && f32::from_bits(self.resonance_bits.load(Ordering::Acquire)).is_finite()
            && runtime.smooth_cutoff.is_finite()
            && runtime.filter_state.iter().all(|value| value.is_finite())
            && self
                .steps
                .iter()
                .all(|step| f32::from_bits(step.load(Ordering::Acquire)).is_finite())
    }
}

// The host serializes prepare/reset with process; pattern edits are atomic.
unsafe impl Sync for StepFilterEngine {}

#[no_mangle]
pub extern "C" fn hirari_step_filter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StepFilterEngine::new(sample_rate))).cast()
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<StepFilterEngine>()) });
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_prepare(state: *const c_void, sample_rate: f64) {
    if let Some(engine) = unsafe { state.cast::<StepFilterEngine>().as_ref() } {
        engine.prepare(sample_rate);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_reset(state: *const c_void) {
    if let Some(engine) = unsafe { state.cast::<StepFilterEngine>().as_ref() } {
        engine.reset();
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_set_step(state: *const c_void, step: u32, value: f32) {
    if let Some(engine) = unsafe { state.cast::<StepFilterEngine>().as_ref() } {
        engine.set_step_value(step, value);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_set_resonance(state: *const c_void, value: f32) {
    if let Some(engine) = unsafe { state.cast::<StepFilterEngine>().as_ref() } {
        engine.set_resonance(value);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_step_filter_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
    bpm: f64,
    context_sample_rate: f64,
    block_start: u64,
) {
    if let Some(engine) = unsafe { state.cast::<StepFilterEngine>().as_ref() } {
        unsafe {
            engine.process(
                channels,
                channel_count,
                frames,
                bpm,
                context_sample_rate,
                block_start,
            );
        }
    }
}
