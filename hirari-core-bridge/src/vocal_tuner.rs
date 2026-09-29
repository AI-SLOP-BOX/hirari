use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocalScale {
    Chromatic,
    Major,
    NaturalMinor,
}

pub struct VocalPitchCorrectorEngine {
    pub sample_rate: f64,
    pub buffer: Vec<f32>,
    pub buffer_left: Vec<f32>,
    pub buffer_right: Vec<f32>,
    correlation: Vec<f32>,
    pub input_pos: usize,
    pub detected_freq: f32,
    pub target_ratio: f32,
    pub phase: f64,
    pub window_size: f32,
    pub amount: f32,
    pub speed: f32,
    pub scale: VocalScale,
}

impl VocalPitchCorrectorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                44_100.0
            },
            buffer: vec![0.0; 8192],
            buffer_left: vec![0.0; 8192],
            buffer_right: vec![0.0; 8192],
            correlation: vec![0.0; 8192],
            input_pos: 0,
            detected_freq: 440.0,
            target_ratio: 1.0,
            phase: 0.0,
            window_size: 1024.0,
            amount: 1.0,
            speed: 0.1,
            scale: VocalScale::Chromatic,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.buffer_left.fill(0.0);
        self.buffer_right.fill(0.0);
        self.correlation.fill(0.0);
        self.detected_freq = 440.0;
        self.target_ratio = 1.0;
        self.phase = 0.0;
        self.input_pos = 0;
    }

    pub fn set_sample_rate(&mut self, sr: f64) {
        if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            self.sample_rate = sr;
            self.reset();
        }
    }

    pub fn set_params(&mut self, amount: f32, speed: f32, scale: VocalScale) {
        if amount.is_finite() {
            self.amount = amount.clamp(0.0, 1.0);
        }
        if speed.is_finite() {
            self.speed = speed.clamp(0.0, 1.0);
        }
        self.scale = scale;
    }

    fn read_buffer(buffer: &[f32], input_pos: usize, phase: f64) -> f32 {
        let mut read_pos = input_pos as f64 - phase;
        while read_pos < 0.0 {
            read_pos += buffer.len() as f64;
        }

        let i1 = read_pos as usize % buffer.len();
        let i2 = (i1 + 1) % buffer.len();
        let frac = (read_pos - read_pos.floor()) as f32;

        (1.0 - frac) * buffer[i1] + frac * buffer[i2]
    }

    /**
     * @brief DETECT: High-stability autocorrelation-based pitch tracking.
     * INDUSTRIAL: Replaces volatile zero-crossing counting with rigorous autocorrelation,
     * suppressing noise, harmonics, and breath sibilance to find true vocal fundamental frequency.
     */
    fn detect_pitch(&mut self) -> f32 {
        let len = self.buffer.len();
        let size = 512; // Window size for correlation
        if len <= size + 2
            || !self.sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&self.sample_rate)
        {
            return self.detected_freq;
        }

        // Human voice pitch ranges between 80Hz and 1000Hz.
        // Lag tau in samples: fs/1000 to fs/80.
        // For 44.1kHz: lag range is ~44 to ~550 samples.
        let min_lag = ((self.sample_rate / 1000.0) as usize).max(1);
        let max_lag = ((self.sample_rate / 80.0) as usize).min(len.saturating_sub(2));
        if min_lag >= max_lag {
            return self.detected_freq;
        }

        self.correlation[..=max_lag].fill(0.0);

        // 1. Calculate autocorrelation for each lag
        for tau in min_lag..=max_lag {
            let mut sum = 0.0f32;
            let mut energy_a = 0.0f32;
            let mut energy_b = 0.0f32;
            for n in 0..size {
                let idx1 = (self.input_pos + len - size - max_lag + n) % len;
                let idx2 = (idx1 + tau) % len;
                let a = self.buffer[idx1];
                let b = self.buffer[idx2];
                sum += a * b;
                energy_a += a * a;
                energy_b += b * b;
            }
            let denominator = (energy_a * energy_b).sqrt();
            self.correlation[tau] = if denominator.is_finite() && denominator > f32::EPSILON {
                sum / denominator
            } else {
                0.0
            };
        }

        // 2. Find the highest local peak inside our lag range
        let mut best_lag = 0;
        let mut max_val = -1e9f32;

        for tau in min_lag..=max_lag {
            // Peak condition: local maximum
            if self.correlation[tau] > max_val
                && self.correlation[tau] > self.correlation[tau - 1]
                && self.correlation[tau] > self.correlation[tau + 1]
            {
                max_val = self.correlation[tau];
                best_lag = tau;
            }
        }

        // Prefer the earliest strong periodic peak. Autocorrelation often
        // rates a subharmonic (e.g. 110 Hz for a 440 Hz tone) slightly higher;
        // selecting the first peak within 92% of the global maximum preserves
        // the fundamental while retaining robustness for breathy voices.
        if best_lag > 0 && max_val.is_finite() {
            let threshold = max_val * 0.92;
            for tau in min_lag..=max_lag {
                if self.correlation[tau] >= threshold
                    && (tau == min_lag || self.correlation[tau] >= self.correlation[tau - 1])
                {
                    best_lag = tau;
                    break;
                }
            }
        }

        if best_lag == 0 {
            return self.detected_freq; // Fallback to previous
        }

        let mut refined_lag = best_lag as f32;
        if best_lag > min_lag && best_lag < max_lag {
            let ym = self.correlation[best_lag - 1];
            let y0 = self.correlation[best_lag];
            let yp = self.correlation[best_lag + 1];
            let denominator = ym - 2.0 * y0 + yp;
            if denominator.is_finite() && denominator.abs() > f32::EPSILON {
                refined_lag += 0.5 * (ym - yp) / denominator;
            }
        }
        let refined_lag = refined_lag.clamp(min_lag as f32, max_lag as f32);
        let freq = self.sample_rate as f32 / refined_lag;
        if (60.0..=1200.0).contains(&freq) {
            freq
        } else {
            self.detected_freq
        }
    }

    /**
     * @brief SCALE: Snaps raw MIDI note frequency to Major, Minor, or Chromatic scale.
     * INDUSTRIAL: Provides correct harmonic snapping context to prevent out-of-scale tuning artifacts.
     */
    fn get_nearest_scale_freq(&self, f: f32) -> f32 {
        if f < 20.0 {
            return 20.0;
        }

        // Convert to absolute MIDI float
        let midi = 12.0 * (f / 440.0).log2() + 69.0;
        let note = midi.round() as i32;
        let octave = note / 12;
        let pitch_in_octave = note % 12;

        // Scale bitmasks
        let mask = match self.scale {
            VocalScale::Chromatic => 0b111111111111,
            VocalScale::Major => 0b101010110101, // C D E F G A B
            VocalScale::NaturalMinor => 0b101101011010, // C D Eb F G Ab Bb
        };

        // Find nearest enabled note
        let mut best_note = pitch_in_octave;
        let mut min_dist = 999;
        for offset in -6..=6 {
            let test_note = (pitch_in_octave + offset + 24) % 12;
            if (mask & (1 << test_note)) != 0 {
                let dist = offset.abs();
                if dist < min_dist {
                    min_dist = dist;
                    best_note = test_note;
                }
            }
        }

        let snapped_midi = (octave * 12 + best_note) as f32;
        440.0 * 2.0f32.powf((snapped_midi - 69.0) / 12.0)
    }

    /// INDUSTRIAL: Performs real-time sample-accurate scale-corrected vocal pitch shifting.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.process_with_parameters(l, Some(r), self.amount, self.speed);
    }

    fn process_with_parameters(
        &mut self,
        l: &mut [f32],
        mut r: Option<&mut [f32]>,
        amount: f32,
        speed: f32,
    ) {
        let len = r.as_ref().map_or(l.len(), |right| l.len().min(right.len()));
        let buffer_len = self.buffer.len();
        if len == 0
            || buffer_len == 0
            || !self.sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&self.sample_rate)
        {
            return;
        }
        if !self.window_size.is_finite()
            || self.window_size < 2.0
            || self.window_size > buffer_len as f32
        {
            self.window_size = 1024.0_f32.min(buffer_len as f32).max(2.0);
            self.phase = 0.0;
        }
        let amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let speed = if speed.is_finite() {
            speed.clamp(0.0, 1.0)
        } else {
            0.0
        };

        for s in 0..len {
            let left_input = if l[s].is_finite() { l[s] } else { 0.0 };
            let right_input = r
                .as_ref()
                .map_or(left_input, |right| if right[s].is_finite() { right[s] } else { 0.0 });
            let in_val = (left_input + right_input) * 0.5;

            // 1. Buffer incoming audio for analysis (20ms windows)
            self.buffer[self.input_pos] = in_val;
            self.buffer_left[self.input_pos] = left_input;
            self.buffer_right[self.input_pos] = right_input;
            self.input_pos = (self.input_pos + 1) % buffer_len;

            // 2. High-stability pitch detection every 512 samples
            if self.input_pos.is_multiple_of(512) {
                self.detected_freq = self.detect_pitch();
            }

            // 3. Compare to scale snapped target frequency
            let nearest_freq = self.get_nearest_scale_freq(self.detected_freq);
            let ratio = (nearest_freq / (self.detected_freq + 1e-9)).clamp(0.25, 4.0);

            // 4. Smooth Ratio (Auto-Tune speed)
            self.target_ratio =
                ((1.0 - speed) * self.target_ratio + speed * ratio).clamp(0.25, 4.0);

            // 5. Dual-tap delay-line pitch shifting with cosine window crossfading
            self.phase += (self.target_ratio - 1.0) as f64;
            if self.phase >= self.window_size as f64 {
                self.phase -= self.window_size as f64;
            }
            if self.phase < 0.0 {
                self.phase += self.window_size as f64;
            }

            let tap1_left = Self::read_buffer(&self.buffer_left, self.input_pos, self.phase);
            let tap2_left = Self::read_buffer(
                &self.buffer_left,
                self.input_pos,
                (self.phase + self.window_size as f64 * 0.5) % self.window_size as f64,
            );
            let tap1_right = Self::read_buffer(&self.buffer_right, self.input_pos, self.phase);
            let tap2_right = Self::read_buffer(
                &self.buffer_right,
                self.input_pos,
                (self.phase + self.window_size as f64 * 0.5) % self.window_size as f64,
            );

            let window1 = 0.5
                * (1.0 - (2.0 * std::f64::consts::PI * self.phase / self.window_size as f64).cos())
                    as f32;
            let window2 = 1.0 - window1;

            let shifted_left = tap1_left * window1 + tap2_left * window2;
            let shifted_right = tap1_right * window1 + tap2_right * window2;

            // Apply correction amount
            let output_left = left_input * (1.0 - amount) + shifted_left * amount;
            let output_right = right_input * (1.0 - amount) + shifted_right * amount;

            l[s] = output_left.clamp(-4.0, 4.0);
            if let Some(right) = r.as_deref_mut() {
                right[s] = output_right.clamp(-4.0, 4.0);
            }
        }
    }

    pub fn audit_vocal_tuner(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && !self.buffer.is_empty()
            && self.input_pos < self.buffer.len()
            && self.detected_freq.is_finite()
            && self.target_ratio.is_finite()
            && self.phase.is_finite()
            && self.amount.is_finite()
            && (0.0..=1.0).contains(&self.amount)
            && self.speed.is_finite()
            && (0.0..=1.0).contains(&self.speed)
    }
}

struct VocalTunerState {
    runtime: UnsafeCell<VocalPitchCorrectorEngine>,
    amount: AtomicU32,
    speed: AtomicU32,
    detected_frequency: AtomicU32,
}

// The DSP runtime has a single serialized audio/lifecycle writer; control
// parameters and the reported pitch are published through atomics.
unsafe impl Sync for VocalTunerState {}

#[no_mangle]
pub extern "C" fn hirari_vocal_tuner_create() -> *mut c_void {
    Box::into_raw(Box::new(VocalTunerState {
        runtime: UnsafeCell::new(VocalPitchCorrectorEngine::new(44_100.0)),
        amount: AtomicU32::new(1.0f32.to_bits()),
        speed: AtomicU32::new(0.1f32.to_bits()),
        detected_frequency: AtomicU32::new(440.0f32.to_bits()),
    })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_destroy(state: *mut c_void) {
    if !state.is_null() { drop(unsafe { Box::from_raw(state.cast::<VocalTunerState>()) }); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_prepare(state: *const c_void, sample_rate: f64) {
    let Some(state) = (unsafe { state.cast::<VocalTunerState>().as_ref() }) else { return; };
    let runtime = unsafe { &mut *state.runtime.get() };
    runtime.set_sample_rate(if sample_rate.is_finite()
        && (8_000.0..=384_000.0).contains(&sample_rate)
    { sample_rate } else { 44_100.0 });
    state.detected_frequency.store(440.0f32.to_bits(), Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_reset(state: *const c_void) {
    let Some(state) = (unsafe { state.cast::<VocalTunerState>().as_ref() }) else { return; };
    unsafe { &mut *state.runtime.get() }.reset();
    state.detected_frequency.store(440.0f32.to_bits(), Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_process(
    state: *const c_void, left: *mut f32, right: *mut f32, frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<VocalTunerState>().as_ref() }) else { return; };
    if left.is_null() || right.is_null() || frames == 0 { return; }
    let amount = f32::from_bits(state.amount.load(Ordering::Relaxed));
    let speed = f32::from_bits(state.speed.load(Ordering::Relaxed));
    let runtime = unsafe { &mut *state.runtime.get() };
    unsafe {
        let left = std::slice::from_raw_parts_mut(left, frames);
        if left.as_mut_ptr() == right {
            runtime.process_with_parameters(left, None, amount, speed);
        } else {
            runtime.process_with_parameters(
                left, Some(std::slice::from_raw_parts_mut(right, frames)), amount, speed,
            );
        }
    }
    state.detected_frequency.store(runtime.detected_freq.to_bits(), Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_set_parameter(
    state: *const c_void, id: u32, value: f32,
) {
    let Some(state) = (unsafe { state.cast::<VocalTunerState>().as_ref() }) else { return; };
    let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { return; };
    match id {
        0 => state.amount.store(value.to_bits(), Ordering::Relaxed),
        1 => state.speed.store(value.to_bits(), Ordering::Relaxed),
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_get_parameter(
    state: *const c_void, id: u32,
) -> f32 {
    let Some(state) = (unsafe { state.cast::<VocalTunerState>().as_ref() }) else { return 0.0; };
    match id {
        0 => f32::from_bits(state.amount.load(Ordering::Relaxed)),
        1 => f32::from_bits(state.speed.load(Ordering::Relaxed)),
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_tuner_detected_frequency(state: *const c_void) -> f32 {
    unsafe { state.cast::<VocalTunerState>().as_ref() }
        .map_or(0.0, |state| f32::from_bits(state.detected_frequency.load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::{VocalPitchCorrectorEngine, VocalScale};

    #[test]
    fn vocal_tuner_keeps_ratio_bounded_and_handles_short_stereo_buffers() {
        let mut tuner = VocalPitchCorrectorEngine::new(48_000.0);
        tuner.set_params(1.0, 1.0, VocalScale::Chromatic);
        let mut left = vec![0.5_f32; 2_048];
        let mut right = vec![0.25_f32; 1_024];
        tuner.process(&mut left, &mut right);
        assert!(left[..1_024]
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite() && sample.abs() <= 4.0));
        assert!(tuner.audit_vocal_tuner());
        assert!((0.25..=4.0).contains(&tuner.target_ratio));
    }

    #[test]
    fn tuner_rejects_nonfinite_parameter_updates() {
        let mut tuner = VocalPitchCorrectorEngine::new(f64::NAN);
        assert!(tuner.audit_vocal_tuner());
        tuner.set_params(f32::NAN, f32::INFINITY, VocalScale::Major);
        assert!(tuner.audit_vocal_tuner());
        tuner.set_params(4.0, -2.0, VocalScale::NaturalMinor);
        assert_eq!(tuner.amount, 1.0);
        assert_eq!(tuner.speed, 0.0);
        tuner.set_sample_rate(96_000.0);
        assert_eq!(tuner.sample_rate, 96_000.0);
        tuner.set_sample_rate(f64::INFINITY);
        assert_eq!(tuner.sample_rate, 96_000.0);
    }

    #[test]
    fn detector_tracks_fractional_period() {
        let mut tuner = VocalPitchCorrectorEngine::new(48_000.0);
        tuner.set_params(0.0, 1.0, VocalScale::Chromatic);
        let mut left = vec![0.0_f32; 4096];
        let mut right = vec![0.0_f32; 4096];
        for (index, sample) in left.iter_mut().enumerate() {
            *sample = (2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0).sin();
        }
        right.copy_from_slice(&left);
        tuner.process(&mut left, &mut right);
        assert!(
            (tuner.detected_freq - 440.0).abs() < 12.0,
            "detected {}",
            tuner.detected_freq
        );
    }
}
