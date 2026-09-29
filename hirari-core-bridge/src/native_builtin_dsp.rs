use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct GateRuntime {
    gain: f32,
    attack: f32,
    release: f32,
}
struct GateState {
    runtime: UnsafeCell<GateRuntime>,
    threshold: AtomicU32,
}
unsafe impl Sync for GateState {}

struct TransientRuntime {
    envelope: f32,
}
struct TransientState {
    runtime: UnsafeCell<TransientRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for TransientState {}

struct DeEsserRuntime {
    detector: f32,
}
struct DeEsserState {
    runtime: UnsafeCell<DeEsserRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for DeEsserState {}

const NATIVE_DELAY_SIZE: usize = 65_536;
struct DelayRuntime {
    sample_rate: f64,
    buffers: [Vec<f32>; 2],
    write_index: usize,
}
struct DelayState {
    runtime: UnsafeCell<DelayRuntime>,
    mix: AtomicU32,
}
unsafe impl Sync for DelayState {}

const NATIVE_REVERB_SIZE: usize = 8192;
struct ReverbRuntime {
    lines: [Vec<f32>; 4],
    index: usize,
}
struct ReverbState {
    runtime: UnsafeCell<ReverbRuntime>,
    mix: AtomicU32,
}
unsafe impl Sync for ReverbState {}

struct DynamicEqRuntime {
    envelope: f32,
}
struct DynamicEqState {
    runtime: UnsafeCell<DynamicEqRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for DynamicEqState {}

struct ScalarProcessorState {
    value: AtomicU32,
}
unsafe impl Sync for ScalarProcessorState {}

#[no_mangle]
pub extern "C" fn hirari_native_noise_gate_create() -> *mut c_void {
    Box::into_raw(Box::new(GateState {
        runtime: UnsafeCell::new(GateRuntime {
            gain: 0.0,
            attack: 0.05,
            release: 0.005,
        }),
        threshold: AtomicU32::new(0.01f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_noise_gate_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<GateState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_noise_gate_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<GateState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.gain = 0.0;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_noise_gate_set_threshold(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<GateState>().as_ref() } {
        state
            .threshold
            .store(value.clamp(0.00001, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_noise_gate_get_threshold(state: *const c_void) -> f32 {
    unsafe { state.cast::<GateState>().as_ref() }.map_or(0.01, |state| {
        f32::from_bits(state.threshold.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_noise_gate_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<GateState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || channel_count == 0 || frames == 0 {
        return;
    }
    let threshold = f32::from_bits(state.threshold.load(Ordering::Relaxed));
    let runtime = unsafe { &mut *state.runtime.get() };
    for frame in 0..frames as usize {
        let mut level = 0.0f32;
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                let sample = unsafe { *samples.add(frame) };
                level = level.max(sample.abs());
            }
        }
        let target = if level >= threshold { 1.0 } else { 0.0 };
        runtime.gain += (target - runtime.gain)
            * if target > runtime.gain {
                runtime.attack
            } else {
                runtime.release
            };
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                let sample = unsafe { samples.add(frame) };
                unsafe { sample.write(*sample * runtime.gain) };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_transient_shaper_create() -> *mut c_void {
    Box::into_raw(Box::new(TransientState {
        runtime: UnsafeCell::new(TransientRuntime { envelope: 0.0 }),
        amount: AtomicU32::new(0.5f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_transient_shaper_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<TransientState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_transient_shaper_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<TransientState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.envelope = 0.0;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_transient_shaper_set_amount(
    state: *const c_void,
    value: f32,
) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<TransientState>().as_ref() } {
        state
            .amount
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_transient_shaper_get_amount(state: *const c_void) -> f32 {
    unsafe { state.cast::<TransientState>().as_ref() }.map_or(0.5, |state| {
        f32::from_bits(state.amount.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_transient_shaper_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<TransientState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || frames == 0 {
        return;
    }
    let amount = f32::from_bits(state.amount.load(Ordering::Relaxed));
    let runtime = unsafe { &mut *state.runtime.get() };
    for frame in 0..frames as usize {
        let mut level = 0.0f32;
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                level = level.max(unsafe { *samples.add(frame) }.abs());
            }
        }
        let transient = (level - runtime.envelope).max(0.0);
        runtime.envelope += (level - runtime.envelope) * 0.08;
        let boost = 1.0 + (transient * amount * 8.0).min(2.0);
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                let sample = unsafe { samples.add(frame) };
                unsafe { sample.write((*sample * boost).clamp(-1.0, 1.0)) };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_deesser_create() -> *mut c_void {
    Box::into_raw(Box::new(DeEsserState {
        runtime: UnsafeCell::new(DeEsserRuntime { detector: 0.0 }),
        amount: AtomicU32::new(0.5f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_deesser_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<DeEsserState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_deesser_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<DeEsserState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.detector = 0.0;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_deesser_set_amount(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<DeEsserState>().as_ref() } {
        state
            .amount
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_deesser_get_amount(state: *const c_void) -> f32 {
    unsafe { state.cast::<DeEsserState>().as_ref() }.map_or(0.5, |state| {
        f32::from_bits(state.amount.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_deesser_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<DeEsserState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || frames == 0 {
        return;
    }
    let amount = f32::from_bits(state.amount.load(Ordering::Relaxed));
    let runtime = unsafe { &mut *state.runtime.get() };
    for frame in 0..frames as usize {
        let mut level = 0.0f32;
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                level = level.max(unsafe { *samples.add(frame) }.abs());
            }
        }
        runtime.detector += (level - runtime.detector) * 0.12;
        let excess = (runtime.detector - 0.45).max(0.0);
        let gain = (1.0 - excess * amount * 2.5).clamp(0.15, 1.0);
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                let sample = unsafe { samples.add(frame) };
                unsafe { sample.write(*sample * gain) };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_delay_create() -> *mut c_void {
    Box::into_raw(Box::new(DelayState {
        runtime: UnsafeCell::new(DelayRuntime {
            sample_rate: 44_100.0,
            buffers: [vec![0.0; NATIVE_DELAY_SIZE], vec![0.0; NATIVE_DELAY_SIZE]],
            write_index: 0,
        }),
        mix: AtomicU32::new(0.35f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<DelayState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_prepare(state: *const c_void, sample_rate: f64) {
    let Some(state) = (unsafe { state.cast::<DelayState>().as_ref() }) else {
        return;
    };
    let runtime = unsafe { &mut *state.runtime.get() };
    runtime.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        44_100.0
    };
    runtime.write_index = 0;
    for channel in &mut runtime.buffers {
        channel.fill(0.0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_reset(state: *const c_void) {
    let Some(state) = (unsafe { state.cast::<DelayState>().as_ref() }) else {
        return;
    };
    let runtime = unsafe { &mut *state.runtime.get() };
    runtime.write_index = 0;
    for channel in &mut runtime.buffers {
        channel.fill(0.0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_set_mix(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<DelayState>().as_ref() } {
        state
            .mix
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_get_mix(state: *const c_void) -> f32 {
    unsafe { state.cast::<DelayState>().as_ref() }.map_or(0.35, |state| {
        f32::from_bits(state.mix.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_delay_process(
    state: *const c_void,
    channel_ptrs: *mut *mut f32,
    channel_count: u32,
    frames: u32,
    bpm: f64,
) {
    let Some(state) = (unsafe { state.cast::<DelayState>().as_ref() }) else {
        return;
    };
    if channel_ptrs.is_null() || frames == 0 {
        return;
    }
    let runtime = unsafe { &mut *state.runtime.get() };
    let tempo = if bpm.is_finite() { bpm.max(1.0) } else { 1.0 };
    let raw_delay = runtime.sample_rate * 60.0 / (tempo * 2.0);
    let delay = (raw_delay as usize).clamp(1, NATIVE_DELAY_SIZE - 1);
    let mix = f32::from_bits(state.mix.load(Ordering::Relaxed));
    let channels = (channel_count as usize).min(2);
    for frame in 0..frames as usize {
        let read = (runtime.write_index + NATIVE_DELAY_SIZE - delay) % NATIVE_DELAY_SIZE;
        for channel in 0..channels {
            let samples = unsafe { *channel_ptrs.add(channel) };
            if samples.is_null() {
                continue;
            }
            let input = unsafe { *samples.add(frame) };
            let echo = runtime.buffers[channel][read];
            runtime.buffers[channel][runtime.write_index] = input + echo * 0.35;
            unsafe { samples.add(frame).write(input * (1.0 - mix) + echo * mix) };
        }
        runtime.write_index = (runtime.write_index + 1) % NATIVE_DELAY_SIZE;
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_reverb_create() -> *mut c_void {
    Box::into_raw(Box::new(ReverbState {
        runtime: UnsafeCell::new(ReverbRuntime {
            lines: std::array::from_fn(|_| vec![0.0; NATIVE_REVERB_SIZE]),
            index: 0,
        }),
        mix: AtomicU32::new(0.3f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_reverb_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ReverbState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_reverb_reset(state: *const c_void) {
    let Some(state) = (unsafe { state.cast::<ReverbState>().as_ref() }) else {
        return;
    };
    let runtime = unsafe { &mut *state.runtime.get() };
    runtime.index = 0;
    for line in &mut runtime.lines {
        line.fill(0.0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_reverb_set_mix(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<ReverbState>().as_ref() } {
        state
            .mix
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_reverb_get_mix(state: *const c_void) -> f32 {
    unsafe { state.cast::<ReverbState>().as_ref() }.map_or(0.3, |state| {
        f32::from_bits(state.mix.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_reverb_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<ReverbState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || channel_count == 0 || frames == 0 {
        return;
    }
    let runtime = unsafe { &mut *state.runtime.get() };
    let mix = f32::from_bits(state.mix.load(Ordering::Relaxed));
    let stereo = channel_count > 1;
    let left = unsafe { *channels };
    let right = if stereo {
        unsafe { *channels.add(1) }
    } else {
        left
    };
    if left.is_null() || right.is_null() {
        return;
    }
    for frame in 0..frames as usize {
        let input_left = unsafe { *left.add(frame) };
        let input_right = if stereo {
            unsafe { *right.add(frame) }
        } else {
            input_left
        };
        let wet_left = runtime.lines[0][runtime.index] + runtime.lines[2][runtime.index];
        let wet_right = runtime.lines[1][runtime.index] + runtime.lines[3][runtime.index];
        runtime.lines[0][runtime.index] = input_left + wet_right * 0.72;
        runtime.lines[1][runtime.index] = input_right + wet_left * 0.72;
        runtime.lines[2][runtime.index] = input_left * 0.7 + wet_left * 0.72 * 0.6;
        runtime.lines[3][runtime.index] = input_right * 0.7 + wet_right * 0.72 * 0.6;
        unsafe {
            left.add(frame)
                .write(input_left * (1.0 - mix) + wet_left * mix * 0.5);
            if stereo {
                right
                    .add(frame)
                    .write(input_right * (1.0 - mix) + wet_right * mix * 0.5);
            }
        }
        runtime.index = (runtime.index + 1) % NATIVE_REVERB_SIZE;
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_dynamic_eq_create() -> *mut c_void {
    Box::into_raw(Box::new(DynamicEqState {
        runtime: UnsafeCell::new(DynamicEqRuntime { envelope: 0.0 }),
        amount: AtomicU32::new(0.5f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_dynamic_eq_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<DynamicEqState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_dynamic_eq_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<DynamicEqState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.envelope = 0.0;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_dynamic_eq_set_amount(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<DynamicEqState>().as_ref() } {
        state
            .amount
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_dynamic_eq_get_amount(state: *const c_void) -> f32 {
    unsafe { state.cast::<DynamicEqState>().as_ref() }.map_or(0.5, |state| {
        f32::from_bits(state.amount.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_dynamic_eq_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<DynamicEqState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || frames == 0 {
        return;
    }
    let runtime = unsafe { &mut *state.runtime.get() };
    let amount = f32::from_bits(state.amount.load(Ordering::Relaxed));
    for frame in 0..frames as usize {
        let mut level = 0.0f32;
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                level = level.max(unsafe { *samples.add(frame) }.abs());
            }
        }
        runtime.envelope += (level - runtime.envelope) * 0.1;
        let reduction = if runtime.envelope > 0.35 {
            ((runtime.envelope - 0.35) * amount).clamp(0.0, 0.75)
        } else {
            0.0
        };
        for channel in 0..(channel_count as usize).min(2) {
            let samples = unsafe { *channels.add(channel) };
            if !samples.is_null() {
                let sample = unsafe { samples.add(frame) };
                unsafe { sample.write(*sample * (1.0 - reduction)) };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_mid_side_create() -> *mut c_void {
    Box::into_raw(Box::new(ScalarProcessorState {
        value: AtomicU32::new(1.0f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_mid_side_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ScalarProcessorState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_mid_side_set(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<ScalarProcessorState>().as_ref() } {
        state
            .value
            .store((value.clamp(0.0, 1.0) * 2.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_mid_side_get(state: *const c_void) -> f32 {
    unsafe { state.cast::<ScalarProcessorState>().as_ref() }.map_or(0.5, |state| {
        f32::from_bits(state.value.load(Ordering::Relaxed)) * 0.5
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_mid_side_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<ScalarProcessorState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || channel_count < 2 {
        return;
    }
    let left = unsafe { *channels };
    let right = unsafe { *channels.add(1) };
    if left.is_null() || right.is_null() {
        return;
    }
    let side_gain = f32::from_bits(state.value.load(Ordering::Relaxed));
    for frame in 0..frames as usize {
        let l = unsafe { *left.add(frame) };
        let r = unsafe { *right.add(frame) };
        let mid = (l + r) * 0.70710678;
        let side = (l - r) * 0.70710678 * side_gain;
        unsafe {
            left.add(frame).write((mid + side) * 0.70710678);
            right.add(frame).write((mid - side) * 0.70710678);
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_stereo_width_create() -> *mut c_void {
    Box::into_raw(Box::new(ScalarProcessorState {
        value: AtomicU32::new(1.0f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_stereo_width_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ScalarProcessorState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_stereo_width_set(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<ScalarProcessorState>().as_ref() } {
        state
            .value
            .store((value.clamp(0.0, 1.0) * 2.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_stereo_width_get(state: *const c_void) -> f32 {
    unsafe { state.cast::<ScalarProcessorState>().as_ref() }.map_or(0.5, |state| {
        f32::from_bits(state.value.load(Ordering::Relaxed)) * 0.5
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_stereo_width_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<ScalarProcessorState>().as_ref() }) else {
        return;
    };
    if channels.is_null() || channel_count < 2 {
        return;
    }
    let left = unsafe { *channels };
    let right = unsafe { *channels.add(1) };
    if left.is_null() || right.is_null() {
        return;
    }
    let width = f32::from_bits(state.value.load(Ordering::Relaxed));
    for frame in 0..frames as usize {
        let l = unsafe { *left.add(frame) };
        let r = unsafe { *right.add(frame) };
        let mid = (l + r) * 0.5;
        let side = (l - r) * 0.5 * width;
        unsafe {
            left.add(frame).write(mid + side);
            right.add(frame).write(mid - side);
        }
    }
}
