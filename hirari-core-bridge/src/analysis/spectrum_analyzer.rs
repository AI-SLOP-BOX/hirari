use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const FFT_SIZE: usize = 1024;
const BAND_COUNT: usize = 64;

pub(crate) struct SpectrumDsp {
    window: [f32; FFT_SIZE],
    bit_reverse: [usize; FFT_SIZE],
    twiddle_real: [f32; FFT_SIZE / 2],
    twiddle_imag: [f32; FFT_SIZE / 2],
    real: [f32; FFT_SIZE],
    imag: [f32; FFT_SIZE],
}

impl SpectrumDsp {
    pub(crate) fn new() -> Self {
        let log2_size = FFT_SIZE.trailing_zeros() as usize;
        let mut window = [0.0; FFT_SIZE];
        let mut bit_reverse = [0; FFT_SIZE];
        let mut twiddle_real = [0.0; FFT_SIZE / 2];
        let mut twiddle_imag = [0.0; FFT_SIZE / 2];
        for index in 0..FFT_SIZE {
            window[index] = (0.5
                * (1.0 - (2.0 * std::f64::consts::PI * index as f64 / (FFT_SIZE - 1) as f64).cos()))
                as f32;
            bit_reverse[index] = Self::reverse_bits(index, log2_size);
        }
        for index in 0..FFT_SIZE / 2 {
            let angle = -2.0 * std::f64::consts::PI * index as f64 / FFT_SIZE as f64;
            twiddle_real[index] = angle.cos() as f32;
            twiddle_imag[index] = angle.sin() as f32;
        }
        Self {
            window,
            bit_reverse,
            twiddle_real,
            twiddle_imag,
            real: [0.0; FFT_SIZE],
            imag: [0.0; FFT_SIZE],
        }
    }

    fn reverse_bits(mut value: usize, count: usize) -> usize {
        let mut reversed = 0;
        for _ in 0..count {
            reversed = (reversed << 1) | (value & 1);
            value >>= 1;
        }
        reversed
    }

    pub(crate) fn process(
        &mut self,
        bands: &[AtomicU32; BAND_COUNT],
        data: &[f32],
        sample_rate: f64,
    ) {
        if data.len() < FFT_SIZE
            || !sample_rate.is_finite()
            || !(1.0..=768_000.0).contains(&sample_rate)
        {
            return;
        }

        for index in 0..FFT_SIZE {
            let sample = if data[index].is_finite() {
                data[index]
            } else {
                0.0
            };
            self.real[index] = sample * self.window[index];
            self.imag[index] = 0.0;
        }
        self.forward();

        let mut new_bands = [0.0_f32; BAND_COUNT];
        for bin in 0..FFT_SIZE / 2 {
            let real = self.real[bin];
            let imag = self.imag[bin];
            let mut magnitude = (real * real + imag * imag).sqrt() / FFT_SIZE as f32;
            if !magnitude.is_finite() {
                magnitude = 0.0;
            }
            let frequency = bin as f32 * sample_rate as f32 / FFT_SIZE as f32;
            if frequency > 20.0 {
                let band = (BAND_COUNT as f32
                    * ((frequency / 20.0).log10() / (20_000.0_f32 / 20.0).log10()))
                    as i32;
                if (0..BAND_COUNT as i32).contains(&band) {
                    let band = band as usize;
                    new_bands[band] = new_bands[band].max(magnitude);
                }
            }
        }

        for index in 0..BAND_COUNT {
            let previous = f32::from_bits(bands[index].load(Ordering::Relaxed));
            let target = new_bands[index];
            let decay = if target > previous { 0.3 } else { 0.05 };
            bands[index].store(
                (previous + (target - previous) * decay).to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    fn forward(&mut self) {
        for index in 0..FFT_SIZE {
            let reverse = self.bit_reverse[index];
            if index < reverse {
                self.real.swap(index, reverse);
                self.imag.swap(index, reverse);
            }
        }
        let mut stage = 1;
        while stage <= FFT_SIZE.trailing_zeros() as usize {
            let span = 1usize << stage;
            let half = span >> 1;
            for block_start in (0..FFT_SIZE).step_by(span) {
                for offset in 0..half {
                    let twiddle = offset * (FFT_SIZE / span);
                    let i1 = block_start + offset;
                    let i2 = i1 + half;
                    let tr = self.twiddle_real[twiddle] * self.real[i2]
                        - self.twiddle_imag[twiddle] * self.imag[i2];
                    let ti = self.twiddle_real[twiddle] * self.imag[i2]
                        + self.twiddle_imag[twiddle] * self.real[i2];
                    self.real[i2] = self.real[i1] - tr;
                    self.imag[i2] = self.imag[i1] - ti;
                    self.real[i1] += tr;
                    self.imag[i1] += ti;
                }
            }
            stage += 1;
        }
    }
}

pub(crate) struct SpectrumState {
    dsp: UnsafeCell<SpectrumDsp>,
    bands: [AtomicU32; BAND_COUNT],
}

impl SpectrumState {
    pub(crate) fn new() -> Self {
        Self {
            dsp: UnsafeCell::new(SpectrumDsp::new()),
            bands: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
        }
    }

    pub(crate) fn process(&self, data: &[f32], sample_rate: f64) {
        let dsp = unsafe { &mut *self.dsp.get() };
        dsp.process(&self.bands, data, sample_rate);
    }

    pub(crate) fn get_band(&self, band: usize) -> f32 {
        if band >= BAND_COUNT {
            return 0.0;
        }
        f32::from_bits(self.bands[band].load(Ordering::Relaxed))
    }
}

#[no_mangle]
pub extern "C" fn hirari_spectrum_analyzer_create() -> *mut c_void {
    Box::into_raw(Box::new(SpectrumState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectrum_analyzer_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SpectrumState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectrum_analyzer_process(
    state: *mut c_void,
    data: *const f32,
    frames: usize,
    sample_rate: f64,
) {
    if state.is_null() || data.is_null() {
        return;
    }
    let handle = &*state.cast::<SpectrumState>();
    handle.process(std::slice::from_raw_parts(data, frames), sample_rate);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectrum_analyzer_get_band(state: *const c_void, band: u32) -> f32 {
    if state.is_null() || band as usize >= BAND_COUNT {
        return 0.0;
    }
    (*state.cast::<SpectrumState>()).get_band(band as usize)
}
