use std::ffi::c_void;

const DELAY_SIZE: usize = 8192;

#[derive(Clone)]
struct DelayModPitchShifter {
    sample_rate: f64,
    delay_buffer: Vec<f32>,
    write_index: usize,
    phase: f32,
    last_output: f32,
}

impl DelayModPitchShifter {
    fn new(sample_rate: f64) -> Self {
        Self { sample_rate, delay_buffer: vec![0.0; DELAY_SIZE], write_index: 0, phase: 0.0, last_output: 0.0 }
    }

    fn reset(&mut self) {
        self.delay_buffer.fill(0.0);
        self.write_index = 0;
        self.phase = 0.0;
        self.last_output = 0.0;
    }

    unsafe fn process(&mut self, input: *const f32, output: *mut f32, samples: u32,
                ratio: f32, f0: f32, sample_rate: f64,
                formant_ratio: f32, timing_ratio: f32) {
        let effective_rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 { sample_rate } else { self.sample_rate };
        let rate_scale = (effective_rate / 48_000.0).clamp(0.5, 2.0) as f32;
        let safe_f0 = if f0.is_finite() && f0 > 20.0 { f0.clamp(20.0, 2_000.0) } else { 140.0 };
        let period = effective_rate as f32 / safe_f0;
        let safe_formant = if formant_ratio.is_finite() { formant_ratio.clamp(0.25, 4.0) } else { 1.0 };
        let safe_timing = if timing_ratio.is_finite() { timing_ratio.clamp(0.25, 4.0) } else { 1.0 };
        let ratio = ratio.clamp(0.5, 2.0);
        let tilt = ((ratio - 1.0) * 0.4 + (safe_formant - 1.0) * 0.1).clamp(-0.8, 0.8);
        let min_delay = (2.0f32 * period).max(256.0 * rate_scale).clamp(256.0, 2048.0);
        let max_delay = (8.0f32 * period).max(min_delay + 256.0).clamp(min_delay + 256.0, 7000.0);
        let delay_range = max_delay - min_delay;
        let phase_speed = (1.0 - ratio) * safe_timing / delay_range;
        let unity = (ratio - 1.0).abs() < 1.0e-6
            && (safe_formant - 1.0).abs() < 1.0e-6
            && (safe_timing - 1.0).abs() < 1.0e-6;

        for i in 0..samples as usize {
            let sample = *input.add(i);
            let out = output.add(i);
            self.delay_buffer[self.write_index] = sample;
            if unity {
                *out = sample;
                self.last_output = sample;
                self.write_index = (self.write_index + 1) % DELAY_SIZE;
                continue;
            }
            self.phase += phase_speed;
            if self.phase >= 1.0 { self.phase -= 1.0; }
            else if self.phase < 0.0 { self.phase += 1.0; }
            let phase_a = self.phase;
            let mut phase_b = self.phase + 0.5;
            if phase_b >= 1.0 { phase_b -= 1.0; }
            let delay_a = min_delay + phase_a * delay_range;
            let delay_b = min_delay + phase_b * delay_range;
            let mut read_a = self.write_index as f32 - delay_a;
            if read_a < 0.0 { read_a += DELAY_SIZE as f32; }
            let mut read_b = self.write_index as f32 - delay_b;
            if read_b < 0.0 { read_b += DELAY_SIZE as f32; }
            let index_a = (read_a as usize) % DELAY_SIZE;
            let next_a = (index_a + 1) % DELAY_SIZE;
            let frac_a = read_a - read_a.floor();
            let sample_a = self.delay_buffer[index_a] * (1.0 - frac_a) + self.delay_buffer[next_a] * frac_a;
            let index_b = (read_b as usize) % DELAY_SIZE;
            let next_b = (index_b + 1) % DELAY_SIZE;
            let frac_b = read_b - read_b.floor();
            let sample_b = self.delay_buffer[index_b] * (1.0 - frac_b) + self.delay_buffer[next_b] * frac_b;
            let win_a = 1.0 - 2.0 * (phase_a - 0.5).abs();
            let win_b = 1.0 - 2.0 * (phase_b - 0.5).abs();
            let shifted = sample_a * win_a + sample_b * win_b;
            self.last_output = shifted + (self.last_output - shifted) * tilt;
            *out = self.last_output;
            self.write_index = (self.write_index + 1) % DELAY_SIZE;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_delay_pitch_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(DelayModPitchShifter::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_delay_pitch_clone(state: *const c_void) -> *mut c_void {
    let Some(state) = state.cast::<DelayModPitchShifter>().as_ref() else { return std::ptr::null_mut() };
    Box::into_raw(Box::new(state.clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_delay_pitch_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<DelayModPitchShifter>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_delay_pitch_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<DelayModPitchShifter>().as_mut() { state.reset(); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_delay_pitch_process(
    state: *mut c_void, input: *const f32, output: *mut f32, samples: u32,
    ratio: f32, f0: f32, sample_rate: f64, formant_ratio: f32, timing_ratio: f32,
) -> bool {
    let Some(state) = state.cast::<DelayModPitchShifter>().as_mut() else { return false };
    if samples == 0 { return true; }
    if input.is_null() || output.is_null() { return false; }
    state.process(input, output, samples, ratio, f0, sample_rate, formant_ratio, timing_ratio);
    true
}
