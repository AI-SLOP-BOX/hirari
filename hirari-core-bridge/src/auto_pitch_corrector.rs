use crate::fft::FftPlan;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct PitchChannel {
    real: Vec<f32>,
    imag: Vec<f32>,
    out_real: Vec<f32>,
    out_imag: Vec<f32>,
    last_phase: Vec<f32>,
    accumulated_phase: Vec<f32>,
}

impl PitchChannel {
    fn new(fft_size: usize) -> Self {
        Self {
            real: vec![0.0; fft_size],
            imag: vec![0.0; fft_size],
            out_real: vec![0.0; fft_size],
            out_imag: vec![0.0; fft_size],
            last_phase: vec![0.0; fft_size / 2 + 1],
            accumulated_phase: vec![0.0; fft_size / 2 + 1],
        }
    }

    fn reset(&mut self) {
        self.last_phase.fill(0.0);
        self.accumulated_phase.fill(0.0);
    }
}

struct AutoPitchCorrector {
    sample_rate: f32,
    fft_size: usize,
    fft: FftPlan,
    left: PitchChannel,
    right: PitchChannel,
    lpc: [f32; 12],
    diff: [f32; 800],
    current_correction: f32,
}

impl AutoPitchCorrector {
    fn new(sample_rate: f64, requested_size: usize) -> Option<Self> {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate as f32
        } else {
            44_100.0
        };
        let fft_size = requested_size
            .clamp(64, 16_384)
            .next_power_of_two()
            .min(16_384);
        Some(Self {
            sample_rate,
            fft_size,
            fft: FftPlan::new(fft_size)?,
            left: PitchChannel::new(fft_size),
            right: PitchChannel::new(fft_size),
            lpc: [0.0; 12],
            diff: [0.0; 800],
            current_correction: 0.0,
        })
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.current_correction = 0.0;
        self.diff.fill(0.0);
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: Option<&mut [f32]>,
        response: f32,
        mask: u32,
    ) -> (f32, f32) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |right| left.len().min(right.len()));
        if frames == 0 {
            return (0.0, 0.0);
        }
        let frequency = self.detect_pitch(left, frames);
        if !(40.0..=2_000.0).contains(&frequency) {
            return (frequency, 0.0);
        }
        let semitone = 12.0 * (frequency / 440.0).log2() + 69.0;
        let target = Self::snap_to_scale(semitone, mask);
        let correction = target - semitone;
        self.current_correction =
            self.current_correction * (1.0 - response) + correction * response;
        let ratio = (2.0f32).powf(self.current_correction / 12.0);
        self.calculate_lpc(left, frames);
        Self::process_channel(
            &self.fft,
            self.fft_size,
            &self.lpc,
            left,
            frames,
            ratio,
            &mut self.left,
        );
        if let Some(right) = right {
            Self::process_channel(
                &self.fft,
                self.fft_size,
                &self.lpc,
                right,
                frames,
                ratio,
                &mut self.right,
            );
        }
        (frequency, self.current_correction)
    }

    fn detect_pitch(&mut self, data: &[f32], frames: usize) -> f32 {
        if frames < 64 || !self.sample_rate.is_finite() {
            return 0.0;
        }
        let tau_max = (frames / 2).min(800);
        let win_size = frames / 2;
        self.diff[..tau_max].fill(0.0);
        for tau in 1..tau_max {
            let mut sum = 0.0f32;
            for i in 0..win_size {
                let difference = data[i] - data[i + tau];
                if difference.is_finite() {
                    sum += difference * difference;
                }
            }
            self.diff[tau] = sum;
        }
        let mut running_sum = 0.0f32;
        self.diff[0] = 1.0;
        for tau in 1..tau_max {
            running_sum += self.diff[tau];
            self.diff[tau] *= tau as f32 / (running_sum + 1.0e-6);
        }
        let mut tau = 20;
        let mut best_tau = 0usize;
        while tau < tau_max {
            if self.diff[tau] < 0.15 {
                while tau + 1 < tau_max && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                best_tau = tau;
                break;
            }
            tau += 1;
        }
        if best_tau == 0 || best_tau + 1 >= tau_max {
            return 0.0;
        }
        let y_left = self.diff[best_tau - 1];
        let y_center = self.diff[best_tau];
        let y_right = self.diff[best_tau + 1];
        let mut shift = (y_right - y_left) / (2.0 * (2.0 * y_center - y_right - y_left) + 1.0e-6);
        if shift.is_finite() {
            shift = shift.clamp(-0.5, 0.5);
        } else {
            shift = 0.0;
        }
        self.sample_rate / (best_tau as f32 + shift)
    }

    fn snap_to_scale(note: f32, mask: u32) -> f32 {
        if mask == 0x0fff || mask == 0 {
            return note;
        }
        let target = note.round() as i32;
        let octave = target.div_euclid(12);
        let semitone = target.rem_euclid(12);
        let mut best_distance = 12;
        let mut best_note = semitone;
        for candidate in 0..12 {
            if mask & (1 << candidate) == 0 {
                continue;
            }
            let raw_distance = (candidate - semitone).abs();
            let distance = raw_distance.min(12 - raw_distance);
            if distance < best_distance {
                best_distance = distance;
                best_note = candidate;
            }
        }
        (octave * 12 + best_note) as f32
    }

    fn calculate_lpc(&mut self, data: &[f32], frames: usize) {
        if frames < 16 {
            return;
        }
        let mut autocorrelation = [0.0f32; 13];
        for lag in 0..=12 {
            for index in 0..frames - lag {
                let a = if data[index].is_finite() {
                    data[index]
                } else {
                    0.0
                };
                let b = if data[index + lag].is_finite() {
                    data[index + lag]
                } else {
                    0.0
                };
                autocorrelation[lag] += a * b;
            }
        }
        if autocorrelation[0] < 1.0e-9 {
            return;
        }
        let mut coefficients = [0.0f32; 13];
        coefficients[0] = 1.0;
        let mut error = autocorrelation[0];
        for order in 1..=12 {
            let mut sum = 0.0;
            for j in 1..order {
                sum += coefficients[j] * autocorrelation[order - j];
            }
            if !error.is_finite() || error < 1.0e-9 {
                break;
            }
            let reflection = ((autocorrelation[order] - sum) / error).clamp(-0.98, 0.98);
            let previous = coefficients;
            coefficients[order] = reflection;
            for j in 1..order {
                coefficients[j] = previous[j] - reflection * previous[order - j];
            }
            error *= 1.0 - reflection * reflection;
            if !error.is_finite() {
                break;
            }
        }
        for index in 0..12 {
            self.lpc[index] = if coefficients[index + 1].is_finite() {
                coefficients[index + 1]
            } else {
                0.0
            };
        }
    }

    fn process_channel(
        fft: &FftPlan,
        fft_size: usize,
        lpc: &[f32; 12],
        data: &mut [f32],
        frames: usize,
        ratio: f32,
        channel: &mut PitchChannel,
    ) {
        if !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        for index in 0..fft_size {
            let window = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * index as f32 / (fft_size - 1) as f32).cos());
            channel.real[index] = if index < frames {
                data[index] * window
            } else {
                0.0
            };
            channel.imag[index] = 0.0;
        }
        fft.forward(&mut channel.real, &mut channel.imag);
        channel.out_real.fill(0.0);
        channel.out_imag.fill(0.0);
        let hop = frames as f32;
        for index in 0..=fft_size / 2 {
            let target = (index as f32 * ratio + 0.5) as usize;
            if target > fft_size / 2 {
                continue;
            }
            let real = channel.real[index];
            let imag = channel.imag[index];
            let magnitude = real.hypot(imag);
            let phase = imag.atan2(real);
            let expected = 2.0 * std::f32::consts::PI * index as f32 * hop / fft_size as f32;
            let mut delta = phase - channel.last_phase[index] - expected;
            while delta > std::f32::consts::PI {
                delta -= 2.0 * std::f32::consts::PI;
            }
            while delta < -std::f32::consts::PI {
                delta += 2.0 * std::f32::consts::PI;
            }
            let true_frequency =
                2.0 * std::f32::consts::PI * index as f32 / fft_size as f32 + delta / hop;
            channel.accumulated_phase[target] += true_frequency * hop * ratio;
            channel.last_phase[index] = phase;
            let env_original = Self::envelope_for(fft_size, index, lpc);
            let env_shifted = Self::envelope_for(fft_size, target, lpc);
            let amplitude = magnitude * env_original / (env_shifted + 1.0e-6);
            channel.out_real[target] += amplitude * channel.accumulated_phase[target].cos();
            channel.out_imag[target] += amplitude * channel.accumulated_phase[target].sin();
        }
        fft.inverse(&mut channel.out_real, &mut channel.out_imag);
        let limit = frames.min(fft_size);
        let normalization = 2.0 / fft_size as f32;
        for (sample, output) in data[..limit].iter_mut().zip(&channel.out_real[..limit]) {
            *sample = *output * normalization;
        }
        data[limit..frames].fill(0.0);
    }

    fn envelope_for(fft_size: usize, bin: usize, lpc: &[f32; 12]) -> f32 {
        let omega = 2.0 * std::f32::consts::PI * bin as f32 / fft_size as f32;
        let mut real = 1.0f32;
        let mut imag = 0.0f32;
        for (index, coefficient) in lpc.iter().enumerate() {
            let phase = -(index as f32 + 1.0) * omega;
            real += coefficient * phase.cos();
            imag += coefficient * phase.sin();
        }
        1.0 / (real.hypot(imag) + 1.0e-6)
    }
}

struct AutoPitchState {
    runtime: UnsafeCell<AutoPitchCorrector>,
    response: AtomicU32,
    scale_mask: AtomicU32,
    detected_frequency: AtomicU32,
    correction_amount: AtomicU32,
}

// The runtime is accessed only by the serialized audio/lifecycle thread.
// Parameter and telemetry fields are atomic and may be accessed concurrently.
unsafe impl Sync for AutoPitchState {}

#[no_mangle]
pub extern "C" fn hirari_auto_pitch_create(sample_rate: f64, fft_size: usize) -> *mut c_void {
    AutoPitchCorrector::new(sample_rate, fft_size).map_or(std::ptr::null_mut(), |runtime| {
        Box::into_raw(Box::new(AutoPitchState {
            runtime: UnsafeCell::new(runtime),
            response: AtomicU32::new(0.5f32.to_bits()),
            scale_mask: AtomicU32::new(0x0fff),
            detected_frequency: AtomicU32::new(0.0f32.to_bits()),
            correction_amount: AtomicU32::new(0.0f32.to_bits()),
        }))
        .cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<AutoPitchState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<AutoPitchState>().as_ref() } {
        let runtime = unsafe { &mut *state.runtime.get() };
        runtime.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate as f32
            } else {
                44_100.0
            };
        runtime.reset();
        state
            .detected_frequency
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        state
            .correction_amount
            .store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<AutoPitchState>().as_ref() } {
        unsafe { &mut *state.runtime.get() }.reset();
        state
            .detected_frequency
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        state
            .correction_amount
            .store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<AutoPitchState>().as_ref() }) else {
        return;
    };
    if left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    unsafe {
        let left = std::slice::from_raw_parts_mut(left, frames);
        if left.as_mut_ptr() == right {
            let (frequency, correction) = (&mut *state.runtime.get()).process(
                left,
                None,
                f32::from_bits(state.response.load(Ordering::Relaxed)),
                state.scale_mask.load(Ordering::Relaxed),
            );
            state
                .detected_frequency
                .store(frequency.to_bits(), Ordering::Relaxed);
            state
                .correction_amount
                .store(correction.to_bits(), Ordering::Relaxed);
        } else {
            let (frequency, correction) = (&mut *state.runtime.get()).process(
                left,
                Some(std::slice::from_raw_parts_mut(right, frames)),
                f32::from_bits(state.response.load(Ordering::Relaxed)),
                state.scale_mask.load(Ordering::Relaxed),
            );
            state
                .detected_frequency
                .store(frequency.to_bits(), Ordering::Relaxed);
            state
                .correction_amount
                .store(correction.to_bits(), Ordering::Relaxed);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    let Some(state) = (unsafe { state.cast::<AutoPitchState>().as_ref() }) else {
        return;
    };
    match id {
        0 => state.response.store(
            (if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.5
            })
            .to_bits(),
            Ordering::Relaxed,
        ),
        1 => state.scale_mask.store(
            if value.is_finite() {
                value.clamp(0.0, 4095.0).round() as u32
            } else {
                0x0fff
            },
            Ordering::Relaxed,
        ),
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_get_parameter(state: *const c_void, id: u32) -> f32 {
    let Some(state) = (unsafe { state.cast::<AutoPitchState>().as_ref() }) else {
        return 0.0;
    };
    match id {
        0 => f32::from_bits(state.response.load(Ordering::Relaxed)),
        1 => state.scale_mask.load(Ordering::Relaxed) as f32,
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_latency(state: *const c_void) -> u32 {
    unsafe { state.cast::<AutoPitchState>().as_ref() }
        .map_or(0, |state| unsafe { (*state.runtime.get()).fft_size as u32 })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_detected_frequency(state: *const c_void) -> f32 {
    unsafe { state.cast::<AutoPitchState>().as_ref() }.map_or(0.0, |state| {
        f32::from_bits(state.detected_frequency.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_pitch_correction_amount(state: *const c_void) -> f32 {
    unsafe { state.cast::<AutoPitchState>().as_ref() }.map_or(0.0, |state| {
        f32::from_bits(state.correction_amount.load(Ordering::Relaxed))
    })
}
