use std::ffi::c_void;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TpdfDitherEngine {
    state: u32,
}

pub struct NoiseShapingDitherEngine {
    state: u32,
    error_history: [f32; 4],
}

impl Default for NoiseShapingDitherEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseShapingDitherEngine {
    pub fn new() -> Self {
        Self {
            state: 0x1234_5678,
            error_history: [0.0; 4],
        }
    }

    fn next_random(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    fn noise(&mut self, bit_step: f32) -> f32 {
        const SCALE: f32 = 1.0 / 4_294_967_295.0;
        let first = self.next_random();
        let second = self.next_random();
        (first as f32 * SCALE + second as f32 * SCALE - 1.0) * bit_step
    }

    pub fn process_sample(&mut self, sample: f32, bits: i32) -> f32 {
        let bits = bits.clamp(1, 32) as u32;
        let bit_step = 1.0 / (1u64 << (bits - 1)) as f32;
        let filtered_error = self.error_history[0] * 2.033 - self.error_history[1] * 2.165
            + self.error_history[2] * 1.259
            - self.error_history[3] * 0.304;
        let input = sample + filtered_error + self.noise(bit_step);
        let quantized = (input / bit_step + 0.5).floor() * bit_step;
        self.error_history.copy_within(0..3, 1);
        self.error_history[0] = input - quantized;
        quantized
    }

    pub fn process(&mut self, samples: &mut [f32], bits: i32) {
        let bits = bits.clamp(1, 32) as u32;
        let bit_step = 1.0 / (1u64 << (bits - 1)) as f32;
        let inverse_bit_step = (1u64 << (bits - 1)) as f32;
        for sample in samples {
            let filtered_error = self.error_history[0] * 2.033 - self.error_history[1] * 2.165
                + self.error_history[2] * 1.259
                - self.error_history[3] * 0.304;
            let input = *sample + filtered_error + self.noise(bit_step);
            let quantized = (input * inverse_bit_step + 0.5).floor() * bit_step;
            self.error_history.copy_within(0..3, 1);
            self.error_history[0] = input - quantized;
            *sample = quantized;
        }
    }
}

impl Default for TpdfDitherEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TpdfDitherEngine {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x1234_5678, |time| {
                time.subsec_nanos() ^ time.as_secs() as u32
            });
        Self {
            state: if seed == 0 { 0x1234_5678 } else { seed },
        }
    }

    #[inline]
    fn next_random(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        const SCALE: f32 = 1.0 / 4_294_967_295.0;
        let first = self.next_random();
        let second = self.next_random();
        let triangular = first as f32 * SCALE + second as f32 * SCALE - 1.0;
        triangular * (1.0 / 8_388_608.0)
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample += self.next_sample();
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_tpdf_dither_create() -> *mut c_void {
    Box::into_raw(Box::new(TpdfDitherEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tpdf_dither_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<TpdfDitherEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tpdf_dither_next(state: *mut c_void) -> f32 {
    state
        .cast::<TpdfDitherEngine>()
        .as_mut()
        .map_or(0.0, TpdfDitherEngine::next_sample)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tpdf_dither_process(
    state: *mut c_void,
    samples: *mut f32,
    frames: usize,
) {
    if samples.is_null() {
        return;
    }
    let Some(state) = state.cast::<TpdfDitherEngine>().as_mut() else {
        return;
    };
    state.process(std::slice::from_raw_parts_mut(samples, frames));
}

#[no_mangle]
pub extern "C" fn hirari_noise_shaping_dither_create() -> *mut c_void {
    Box::into_raw(Box::new(NoiseShapingDitherEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_noise_shaping_dither_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<NoiseShapingDitherEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_noise_shaping_dither_process_sample(
    state: *mut c_void,
    sample: f32,
    bits: i32,
) -> f32 {
    state
        .cast::<NoiseShapingDitherEngine>()
        .as_mut()
        .map_or(sample, |state| state.process_sample(sample, bits))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_noise_shaping_dither_process(
    state: *mut c_void,
    samples: *mut f32,
    frames: usize,
    bits: i32,
) {
    if samples.is_null() {
        return;
    }
    let Some(state) = state.cast::<NoiseShapingDitherEngine>().as_mut() else {
        return;
    };
    state.process(std::slice::from_raw_parts_mut(samples, frames), bits);
}
