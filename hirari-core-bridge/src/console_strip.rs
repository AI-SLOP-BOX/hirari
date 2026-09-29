use std::ffi::c_void;

#[derive(Clone, Copy)]
struct BellFilter {
    frequency: f32,
    gain_db: f32,
    q: f32,
    gain: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    s1: [f32; 2],
    s2: [f32; 2],
}

impl BellFilter {
    fn new(sample_rate: f64, frequency: f32, gain_db: f32, q: f32) -> Self {
        let mut filter = Self {
            frequency,
            gain_db,
            q,
            gain: 1.0,
            k: 0.0,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        };
        filter.set_params(sample_rate, frequency, gain_db, q);
        filter
    }

    fn set_params(&mut self, sample_rate: f64, frequency: f32, gain_db: f32, q: f32) {
        self.frequency = frequency;
        self.gain_db = gain_db;
        self.q = q;
        let frequency = frequency.clamp(5.0, (sample_rate * 0.45) as f32);
        let q = q.clamp(0.05, 20.0);
        let g = (std::f32::consts::PI * frequency / sample_rate as f32).tan();
        self.k = 1.0 / q;
        self.gain = 10.0f32.powf(gain_db / 40.0);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    fn process(&mut self, data: &mut [f32], channel: usize) {
        for sample in data {
            let input = if sample.is_finite() { *sample } else { 0.0 };
            let v3 = input - self.s2[channel];
            let v1 = self.a1 * self.s1[channel] + self.a2 * v3;
            let v2 = self.s2[channel] + self.a2 * self.s1[channel] + self.a3 * v3;
            self.s1[channel] = 2.0 * v1 - self.s1[channel];
            self.s2[channel] = 2.0 * v2 - self.s2[channel];
            let output = input + (self.gain - 1.0) * v1;
            *sample = if output.is_finite() { output } else { 0.0 };
        }
    }

    fn reset(&mut self) {
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }
}

/// Rust-owned DSP state for the native DivineConsoleStrip processor.
pub struct DivineConsoleStripEngine {
    sample_rate: f64,
    input_gain: f32,
    output_gain: f32,
    threshold_db: f32,
    envelope: [f32; 2],
    bands: [BellFilter; 4],
}

impl DivineConsoleStripEngine {
    fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1000.0 {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            sample_rate,
            input_gain: 1.0,
            output_gain: 1.0,
            threshold_db: -20.0,
            envelope: [0.0; 2],
            bands: [
                BellFilter::new(sample_rate, 90.0, 0.0, 0.8),
                BellFilter::new(sample_rate, 700.0, 0.0, 0.9),
                BellFilter::new(sample_rate, 3200.0, 0.0, 0.9),
                BellFilter::new(sample_rate, 10_000.0, 0.0, 0.8),
            ],
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 1000.0 {
            sample_rate
        } else {
            44_100.0
        };
        for band in &mut self.bands {
            let (frequency, gain_db, q) = (band.frequency, band.gain_db, band.q);
            band.set_params(self.sample_rate, frequency, gain_db, q);
        }
    }

    fn set_band(&mut self, index: usize, frequency: f32, gain_db: f32, q: f32) -> bool {
        if index >= self.bands.len()
            || !frequency.is_finite()
            || !gain_db.is_finite()
            || !q.is_finite()
            || frequency <= 5.0
            || q <= 0.05
        {
            return false;
        }
        self.bands[index].set_params(
            self.sample_rate,
            frequency,
            gain_db.clamp(-24.0, 24.0),
            q.clamp(0.05, 20.0),
        );
        true
    }

    fn reset(&mut self) {
        self.envelope = [0.0; 2];
        for band in &mut self.bands {
            band.reset();
        }
    }

    fn process(&mut self, channels: &mut [*mut f32], frames: usize) {
        for (channel, data) in channels.iter_mut().take(2).enumerate() {
            if data.is_null() {
                continue;
            }
            // SAFETY: The C++ caller passes writable channel buffers with at least `frames` samples.
            let samples = unsafe { std::slice::from_raw_parts_mut(*data, frames) };
            let mut envelope = self.envelope[channel];
            for sample in samples.iter_mut() {
                let input = if sample.is_finite() { *sample } else { 0.0 };
                let driven = input * self.input_gain;
                let saturated = (driven + 0.15 * driven * driven * 1.0f32.copysign(driven))
                    / (1.0 + 0.35 * driven.abs());
                envelope += (saturated.abs() - envelope) * 0.01;
                let over = (20.0 * envelope.max(1.0e-6).log10() - self.threshold_db).max(0.0);
                let gain = 10.0f32.powf(-(over * 0.25).min(12.0) / 20.0);
                let output = saturated * gain * self.output_gain;
                *sample = if output.is_finite() { output } else { 0.0 };
            }
            self.envelope[channel] = envelope;
        }

        for band in &mut self.bands {
            for (channel, data) in channels.iter_mut().take(2).enumerate() {
                if data.is_null() {
                    continue;
                }
                // SAFETY: Same writable channel slices as above; bands process in sequence.
                band.process(
                    unsafe { std::slice::from_raw_parts_mut(*data, frames) },
                    channel,
                );
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_console_strip_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(DivineConsoleStripEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<DivineConsoleStripEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
) {
    if let Some(state) = state.cast::<DivineConsoleStripEngine>().as_mut() {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_set_input_gain(state: *mut c_void, gain: f32) {
    if gain.is_finite() {
        if let Some(state) = state.cast::<DivineConsoleStripEngine>().as_mut() {
            state.input_gain = gain;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_set_output_gain(state: *mut c_void, gain: f32) {
    if gain.is_finite() {
        if let Some(state) = state.cast::<DivineConsoleStripEngine>().as_mut() {
            state.output_gain = gain;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_set_threshold(state: *mut c_void, db: f32) {
    if db.is_finite() {
        if let Some(state) = state.cast::<DivineConsoleStripEngine>().as_mut() {
            state.threshold_db = db;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_set_band(
    state: *mut c_void,
    index: u32,
    frequency: f32,
    gain_db: f32,
    q: f32,
) -> bool {
    state
        .cast::<DivineConsoleStripEngine>()
        .as_mut()
        .is_some_and(|state| state.set_band(index as usize, frequency, gain_db, q))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<DivineConsoleStripEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_console_strip_process(
    state: *mut c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: usize,
) {
    let (Some(state), Some(channels)) = (
        state.cast::<DivineConsoleStripEngine>().as_mut(),
        (!channels.is_null())
            .then(|| std::slice::from_raw_parts_mut(channels, channel_count as usize)),
    ) else {
        return;
    };
    state.process(channels, frames);
}
