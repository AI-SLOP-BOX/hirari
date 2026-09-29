use std::ffi::c_void;

const CHANNELS: usize = 12;
const SAMPLE_RATE_MIN: f64 = 8_000.0;
const SAMPLE_RATE_MAX: f64 = 384_000.0;

/// Rust-owned state for the 12-channel feedback delay network used by the
/// immersive reverb processor.
pub struct AtmosReverbEngine {
    sample_rate: f64,
    delay_lines: [Vec<f32>; CHANNELS],
    write_indices: [usize; CHANNELS],
    filter_state: [f32; CHANNELS],
}

impl AtmosReverbEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            delay_lines: std::array::from_fn(|_| Vec::new()),
            write_indices: [0; CHANNELS],
            filter_state: [0.0; CHANNELS],
        };
        engine.set_sample_rate(sample_rate);
        engine
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite()
            && (SAMPLE_RATE_MIN..=SAMPLE_RATE_MAX).contains(&sample_rate)
        {
            sample_rate
        } else {
            44_100.0
        };
        let delay_samples = ((self.sample_rate * 0.1) as usize).max(1);
        for delay in &mut self.delay_lines {
            delay.resize(delay_samples, 0.0);
            delay.fill(0.0);
        }
        self.write_indices.fill(0);
    }

    pub fn reset(&mut self) {
        for delay in &mut self.delay_lines {
            delay.fill(0.0);
        }
        self.write_indices.fill(0);
        self.filter_state.fill(0.0);
    }

    pub fn tail_samples(&self) -> u32 {
        let longest = self.delay_lines[0].len();
        (longest
            .saturating_mul(40)
            .min((self.sample_rate * 30.0) as usize)) as u32
    }

    /// Process channel pointers directly so the audio callback allocates no
    /// slices or buffers and nullable channel slots retain the old C++ rules.
    unsafe fn process_raw(&mut self, buffers: *const *mut f32, buffer_count: u32, frames: u32) {
        let channels = (buffer_count as usize).min(CHANNELS);
        if channels == 0 || frames == 0 || buffers.is_null() {
            return;
        }

        for frame in 0..frames as usize {
            let mut input = 0.0f32;
            let mut active = 0u32;
            for channel in 0..channels {
                let buffer = unsafe { *buffers.add(channel) };
                if !buffer.is_null() {
                    let sample = unsafe { *buffer.add(frame) };
                    input += if sample.is_finite() { sample } else { 0.0 };
                    active += 1;
                }
            }
            if active == 0 {
                continue;
            }
            input /= active as f32;

            let mut sum = 0.0f32;
            for channel in 0..CHANNELS {
                let delay = &self.delay_lines[channel];
                let read = (self.write_indices[channel] + 1) % delay.len();
                let value = delay[read];
                let value = if value.is_finite() { value } else { 0.0 };
                self.filter_state[channel] += (value - self.filter_state[channel]) * 0.08;
                sum += self.filter_state[channel];
            }
            let mean = sum / CHANNELS as f32;

            for channel in 0..CHANNELS {
                let fed = input * 0.18 + (self.filter_state[channel] - 2.0 * mean) * 0.82;
                let fed = if fed.is_finite() { fed } else { 0.0 };
                self.delay_lines[channel][self.write_indices[channel]] = fed;
                self.write_indices[channel] =
                    (self.write_indices[channel] + 1) % self.delay_lines[channel].len();
            }

            for channel in 0..channels {
                let buffer = unsafe { *buffers.add(channel) };
                if buffer.is_null() {
                    continue;
                }
                let wet = self.filter_state[channel] * 0.35 + mean * 0.15;
                let sample_ptr = unsafe { buffer.add(frame) };
                let raw_dry = unsafe { *sample_ptr };
                let dry = if raw_dry.is_finite() { raw_dry } else { 0.0 };
                let output = dry * 0.8 + wet * 0.2;
                unsafe {
                    *sample_ptr = if output.is_finite() {
                        output.clamp(-16.0, 16.0)
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    pub fn audit(&self) -> bool {
        self.sample_rate.is_finite()
            && (SAMPLE_RATE_MIN..=SAMPLE_RATE_MAX).contains(&self.sample_rate)
            && self.delay_lines.iter().all(|line| !line.is_empty())
            && self
                .write_indices
                .iter()
                .zip(self.delay_lines.iter())
                .all(|(index, line)| *index < line.len())
            && self.filter_state.iter().all(|state| state.is_finite())
            && self
                .delay_lines
                .iter()
                .flatten()
                .all(|sample| sample.is_finite())
    }
}

#[no_mangle]
pub extern "C" fn hirari_atmos_reverb_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(AtmosReverbEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmos_reverb_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle was allocated by `hirari_atmos_reverb_create`.
        unsafe { drop(Box::from_raw(state.cast::<AtmosReverbEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmos_reverb_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<AtmosReverbEngine>().as_mut() } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmos_reverb_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<AtmosReverbEngine>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmos_reverb_tail_samples(state: *const c_void) -> u32 {
    unsafe { state.cast::<AtmosReverbEngine>().as_ref() }.map_or(0, |state| state.tail_samples())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmos_reverb_process(
    state: *mut c_void,
    buffers: *const *mut f32,
    buffer_count: u32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<AtmosReverbEngine>().as_mut() }) else {
        return;
    };
    // SAFETY: caller provides up to `buffer_count` pointers with `frames`
    // samples each. The engine only dereferences non-null channel pointers.
    unsafe { state.process_raw(buffers, buffer_count, frames) };
}

#[cfg(test)]
mod tests {
    use super::{AtmosReverbEngine, CHANNELS};

    #[test]
    fn stereo_render_matches_legacy_dry_and_wet_equations() {
        let mut engine = AtmosReverbEngine::new(8_000.0);
        let mut left = [1.0f32, 0.5, -0.25, f32::NAN];
        let mut right = [0.25f32, -0.5, 0.75, 0.0];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe { engine.process_raw(channels.as_mut_ptr(), channels.len() as u32, 4) };
        assert_eq!(left[0], 0.8);
        assert_eq!(right[0], 0.2);
        assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
        assert!(engine.audit());
    }

    #[test]
    fn mismatched_null_slots_and_audio_block_boundaries_keep_state_valid() {
        let mut engine = AtmosReverbEngine::new(8_000.0);
        let mut left = [0.0f32; 24];
        left[0] = 0.75;
        let mut channels = [left.as_mut_ptr(), std::ptr::null_mut()];
        unsafe { engine.process_raw(channels.as_mut_ptr(), 2, 9) };
        unsafe { engine.process_raw(channels.as_mut_ptr(), 2, 15) };
        assert!(engine.audit());
        assert_eq!(engine.delay_lines.len(), CHANNELS);
        assert_eq!(engine.delay_lines[0].len(), 800);
        assert_eq!(engine.tail_samples(), 32_000);
    }
}
