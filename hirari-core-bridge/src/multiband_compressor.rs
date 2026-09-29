use std::ffi::c_void;

pub struct BandComp {
    pub env: f32,
    pub gain: f32,
    pub sample_rate: f64,
}

impl BandComp {
    pub fn new(sr: f64) -> Self {
        Self {
            env: 0.0,
            gain: 1.0,
            sample_rate: sr,
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.env = 0.0;
    }

    pub fn process(&mut self, peak: f32) -> f32 {
        // Standard Compressor Logic: Threshold at -12dBFS
        let threshold = 0.25;
        let ratio = 4.0;

        let mut target_gain = 1.0;
        if peak > threshold {
            target_gain = (threshold / peak).powf(1.0 - 1.0 / ratio);
        }

        // Exponential Envelope Follower (Professional Grade)
        let sample_rate = self.sample_rate as f32;
        let attack = (-1.0f32 / (0.010f32 * sample_rate)).exp(); // 10ms
        let release = (-1.0f32 / (0.100f32 * sample_rate)).exp(); // 100ms

        let coeff = if target_gain < self.gain {
            attack
        } else {
            release
        };
        self.gain = coeff * self.gain + (1.0 - coeff) * target_gain;

        self.gain
    }
}

pub struct MultibandCompressorEngine {
    pub sample_rate: f64,
    pub low_mid_freq: f32,
    pub mid_high_freq: f32,
    pub low_band_unit: BandComp,
    pub mid_band_unit: BandComp,
    pub high_band_unit: BandComp,
    pub filters: [f32; 8],
}

impl MultibandCompressorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            low_mid_freq: 200.0,
            mid_high_freq: 2500.0,
            low_band_unit: BandComp::new(sr),
            mid_band_unit: BandComp::new(sr),
            high_band_unit: BandComp::new(sr),
            filters: [0.0; 8],
        }
    }

    pub fn reset(&mut self) {
        self.low_band_unit.reset();
        self.mid_band_unit.reset();
        self.high_band_unit.reset();
        self.filters.fill(0.0);
    }

    pub fn set_split_freqs(&mut self, low_mid: f32, mid_high: f32) {
        self.low_mid_freq = cpp_clamp(low_mid, 20.0, 10_000.0);
        self.mid_high_freq = cpp_clamp(mid_high, self.low_mid_freq + 20.0, 20_000.0);
    }

    fn process_lpf(&mut self, input: f32, freq: f32, idx: usize) -> f32 {
        let alpha = freq / (freq + self.sample_rate as f32);
        self.filters[idx] += alpha * (input - self.filters[idx]);
        self.filters[idx]
    }

    fn process_hpf(&mut self, input: f32, freq: f32, idx: usize) -> f32 {
        let alpha = freq / (freq + self.sample_rate as f32);
        self.filters[idx + 4] += alpha * (input - self.filters[idx + 4]);
        input - self.filters[idx + 4]
    }

    fn process_channel(&mut self, input: f32, channel: usize) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        let low = self.process_lpf(input, self.low_mid_freq, channel);
        let high = self.process_hpf(input, self.mid_high_freq, channel);
        let mid = input - low - high;
        let gain_low = self.low_band_unit.process(low.abs());
        let gain_mid = self.mid_band_unit.process(mid.abs());
        let gain_high = self.high_band_unit.process(high.abs());
        let output = low * gain_low + mid * gain_mid + high * gain_high;
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }

    /// INDUSTRIAL: 3-Band Dynamics Processor with 1st-order complementary crossover.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if len == 0 {
            return;
        }

        for s in 0..len {
            l[s] = self.process_channel(l[s], 0);
            r[s] = self.process_channel(r[s], 1);
        }
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || !(100.0..=384_000.0).contains(&sample_rate) {
            return;
        }
        self.sample_rate = sample_rate;
        self.low_band_unit.sample_rate = sample_rate;
        self.mid_band_unit.sample_rate = sample_rate;
        self.high_band_unit.sample_rate = sample_rate;
    }

    pub fn tail_samples(&self) -> u32 {
        (0.8 * self.sample_rate.clamp(100.0, 384_000.0)) as u32
    }

    /// # Safety
    /// Every non-null channel pointer must be writable for `frames` samples.
    pub unsafe fn process_channel_pointers(&mut self, channels: &[*mut f32], frames: usize) {
        let count = channels.len().min(2);
        for frame in 0..frames {
            for (channel, pointer) in channels.iter().take(count).enumerate() {
                if pointer.is_null() {
                    continue;
                }
                // SAFETY: guaranteed by this method's caller contract.
                let sample = unsafe { pointer.add(frame).read() };
                let output = self.process_channel(sample, channel);
                // SAFETY: guaranteed by this method's caller contract.
                unsafe { pointer.add(frame).write(output) };
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Multiband Compressor state.
    pub fn audit_multiband_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && (100.0..=384_000.0).contains(&self.sample_rate)
            && self.low_mid_freq.is_finite()
            && self.mid_high_freq.is_finite()
            && (20.0..=10_000.0).contains(&self.low_mid_freq)
            && (self.low_mid_freq + 20.0..=20_000.0).contains(&self.mid_high_freq)
            && self
                .filters
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 4.0)
            && [
                &self.low_band_unit,
                &self.mid_band_unit,
                &self.high_band_unit,
            ]
            .iter()
            .all(|band| {
                band.sample_rate.is_finite()
                    && (100.0..=384_000.0).contains(&band.sample_rate)
                    && band.env.is_finite()
                    && (0.0..=4.0).contains(&band.env)
                    && band.gain.is_finite()
                    && (0.0..=1.0).contains(&band.gain)
            })
    }
}

fn cpp_clamp(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

#[no_mangle]
pub extern "C" fn hirari_multiband_compressor_create(sample_rate: f64) -> *mut c_void {
    let mut engine = MultibandCompressorEngine::new(sample_rate);
    engine.set_split_freqs(200.0, 2500.0);
    Box::into_raw(Box::new(engine)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle was allocated by `hirari_multiband_compressor_create`.
        unsafe { drop(Box::from_raw(state.cast::<MultibandCompressorEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<MultibandCompressorEngine>().as_mut() } {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<MultibandCompressorEngine>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_set_split_freqs(
    state: *mut c_void,
    low_mid: f32,
    mid_high: f32,
) {
    if let Some(state) = unsafe { state.cast::<MultibandCompressorEngine>().as_mut() } {
        state.set_split_freqs(low_mid, mid_high);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_process(
    state: *mut c_void,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) {
    if channels.is_null() || channel_count == 0 || frames == 0 {
        return;
    }
    let Some(state) = (unsafe { state.cast::<MultibandCompressorEngine>().as_mut() }) else {
        return;
    };
    let channels = unsafe { std::slice::from_raw_parts(channels, channel_count.min(2) as usize) };
    // SAFETY: pointers originate from the host AudioBuffer and have `frames` samples.
    unsafe { state.process_channel_pointers(channels, frames as usize) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_compressor_tail(state: *const c_void) -> u32 {
    unsafe { state.cast::<MultibandCompressorEngine>().as_ref() }
        .map_or(0, MultibandCompressorEngine::tail_samples)
}

#[cfg(test)]
mod tests {
    use super::MultibandCompressorEngine;

    #[test]
    fn multiband_processing_is_stereo_safe_and_finite() {
        let mut engine = MultibandCompressorEngine::new(48_000.0);
        engine.set_split_freqs(180.0, 2_400.0);
        let mut left = vec![0.8_f32; 512];
        let mut right = vec![0.4_f32; 256];
        engine.process(&mut left, &mut right);
        assert!(left[..256]
            .iter()
            .chain(right.iter())
            .all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(engine.audit_multiband_compressor());
    }

    #[test]
    fn multiband_audit_rejects_corrupt_state_and_split_updates() {
        let mut engine = MultibandCompressorEngine::new(48_000.0);
        engine.set_split_freqs(20_000.0, 100.0);
        assert_eq!(engine.low_mid_freq, 10_000.0);
        assert_eq!(engine.mid_high_freq, 10_020.0);
        engine.filters[0] = f32::NAN;
        assert!(!engine.audit_multiband_compressor());
    }

    struct LegacyBand {
        gain: f32,
        sample_rate: f32,
    }

    impl LegacyBand {
        fn process(&mut self, peak: f32) -> f32 {
            let threshold = 0.25f32;
            let ratio = 4.0f32;
            let mut target_gain = 1.0f32;
            if peak > threshold {
                target_gain = (threshold / peak).powf(1.0 - 1.0 / ratio);
            }
            let attack = (-1.0f32 / (0.010f32 * self.sample_rate)).exp();
            let release = (-1.0f32 / (0.100f32 * self.sample_rate)).exp();
            let coefficient = if target_gain < self.gain {
                attack
            } else {
                release
            };
            self.gain = coefficient * self.gain + (1.0 - coefficient) * target_gain;
            self.gain
        }
    }

    struct LegacyCppReference {
        sample_rate: f32,
        low_mid: f32,
        mid_high: f32,
        filters: [f32; 8],
        bands: [LegacyBand; 3],
    }

    impl LegacyCppReference {
        fn new(sample_rate: f32) -> Self {
            Self {
                sample_rate,
                low_mid: 180.0,
                mid_high: 2400.0,
                filters: [0.0; 8],
                bands: std::array::from_fn(|_| LegacyBand {
                    gain: 1.0,
                    sample_rate,
                }),
            }
        }

        fn process_channel(&mut self, input: f32, channel: usize) -> f32 {
            let input = if input.is_finite() { input } else { 0.0 };
            let alpha_low = self.low_mid / (self.low_mid + self.sample_rate);
            self.filters[channel] += alpha_low * (input - self.filters[channel]);
            let low = self.filters[channel];
            let alpha_high = self.mid_high / (self.mid_high + self.sample_rate);
            self.filters[channel + 4] += alpha_high * (input - self.filters[channel + 4]);
            let high = input - self.filters[channel + 4];
            let mid = input - low - high;
            let gain_low = self.bands[0].process(low.abs());
            let gain_mid = self.bands[1].process(mid.abs());
            let gain_high = self.bands[2].process(high.abs());
            let output = low * gain_low + mid * gain_mid + high * gain_high;
            if output.is_finite() {
                output
            } else {
                0.0
            }
        }

        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            for frame in 0..left.len().min(right.len()) {
                left[frame] = self.process_channel(left[frame], 0);
                right[frame] = self.process_channel(right[frame], 1);
            }
        }
    }

    #[test]
    fn rust_ffi_processing_matches_the_legacy_cpp_channel_order() {
        let state = super::hirari_multiband_compressor_create(48_000.0);
        assert!(!state.is_null());
        unsafe { super::hirari_multiband_compressor_set_split_freqs(state, 180.0, 2400.0) };
        let mut left = (0..4096)
            .map(|frame| (frame as f32 * 0.071).sin() * 0.9 + (frame % 23) as f32 * 0.01)
            .collect::<Vec<_>>();
        let mut right = (0..4096)
            .map(|frame| (frame as f32 * 0.037).cos() * 0.7 - (frame % 31) as f32 * 0.008)
            .collect::<Vec<_>>();
        left[517] = f32::NAN;
        right[1024] = f32::INFINITY;
        let mut reference_left = left.clone();
        let mut reference_right = right.clone();
        let mut reference = LegacyCppReference::new(48_000.0);
        for start in (0..left.len()).step_by(127) {
            let end = (start + 127).min(left.len());
            let pointers = [left[start..].as_mut_ptr(), right[start..].as_mut_ptr()];
            unsafe {
                super::hirari_multiband_compressor_process(
                    state,
                    pointers.as_ptr(),
                    pointers.len() as u32,
                    (end - start) as u32,
                );
            }
            reference.process(
                &mut reference_left[start..end],
                &mut reference_right[start..end],
            );
        }
        for (actual, expected) in left.iter().zip(&reference_left) {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "{actual} != {expected}"
            );
        }
        for (actual, expected) in right.iter().zip(&reference_right) {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "{actual} != {expected}"
            );
        }
        unsafe { super::hirari_multiband_compressor_destroy(state) };
    }
}
