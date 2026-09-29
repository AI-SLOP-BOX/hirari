use crate::fft::FftPlan;
use std::ffi::c_void;
use std::slice;

const PARTITION_SIZE: usize = 512;
const FFT_SIZE: usize = PARTITION_SIZE * 2;
const MAX_PARTITIONS: usize = 32;
type Spectrum = [f32; FFT_SIZE];

struct ConvolutionReverb {
    fft: FftPlan,
    sample_rate: f32,
    segments_real: Vec<Spectrum>,
    segments_imag: Vec<Spectrum>,
    impulse_real: Vec<Spectrum>,
    impulse_imag: Vec<Spectrum>,
    history: [f32; PARTITION_SIZE],
    overlap: [f32; PARTITION_SIZE],
    scratch_real: Spectrum,
    scratch_imag: Spectrum,
    write_index: usize,
}

impl ConvolutionReverb {
    fn new(sample_rate: f64) -> Option<Self> {
        let mut reverb = Self {
            fft: FftPlan::new(FFT_SIZE)?,
            sample_rate: Self::valid_sample_rate(sample_rate),
            segments_real: vec![[0.0; FFT_SIZE]; MAX_PARTITIONS],
            segments_imag: vec![[0.0; FFT_SIZE]; MAX_PARTITIONS],
            impulse_real: vec![[0.0; FFT_SIZE]; MAX_PARTITIONS],
            impulse_imag: vec![[0.0; FFT_SIZE]; MAX_PARTITIONS],
            history: [0.0; PARTITION_SIZE],
            overlap: [0.0; PARTITION_SIZE],
            scratch_real: [0.0; FFT_SIZE],
            scratch_imag: [0.0; FFT_SIZE],
            write_index: 0,
        };
        reverb.set_ir(0);
        Some(reverb)
    }

    fn valid_sample_rate(sample_rate: f64) -> f32 {
        if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
            sample_rate as f32
        } else {
            44_100.0
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = Self::valid_sample_rate(sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.write_index = 0;
        self.history.fill(0.0);
        self.overlap.fill(0.0);
        for segment in &mut self.segments_real {
            segment.fill(0.0);
        }
        for segment in &mut self.segments_imag {
            segment.fill(0.0);
        }
    }

    fn set_ir(&mut self, model: u32) {
        let concrete = model == 1;
        let decay = if concrete { 3.6 } else { 1.8 };
        let diffuse = if concrete { 0.06 } else { 0.035 };
        let rate = if self.sample_rate.is_finite() && self.sample_rate >= 8_000.0 {
            self.sample_rate
        } else {
            48_000.0
        };
        let mut random = 0x1234_u32;
        for partition in 0..MAX_PARTITIONS {
            self.impulse_real[partition].fill(0.0);
            self.impulse_imag[partition].fill(0.0);
            for frame in 0..PARTITION_SIZE {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let time = (partition * PARTITION_SIZE + frame) as f32;
                let envelope = (-time / (decay * rate)).exp();
                let noise = (random & 0xffff) as f32 / 32_767.5 - 1.0;
                let early = if partition == 0 && frame == 0 {
                    0.85
                } else {
                    0.0
                };
                self.impulse_real[partition][frame] = early + diffuse * envelope * noise;
            }
            self.fft.forward(
                &mut self.impulse_real[partition],
                &mut self.impulse_imag[partition],
            );
        }
    }

    fn load_ir(&mut self, samples: &[f32]) -> bool {
        if samples.is_empty()
            || samples.len() > MAX_PARTITIONS * PARTITION_SIZE
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return false;
        }
        for partition in 0..MAX_PARTITIONS {
            self.impulse_real[partition].fill(0.0);
            self.impulse_imag[partition].fill(0.0);
            let start = partition * PARTITION_SIZE;
            let end = samples.len().min(start + PARTITION_SIZE);
            if start < end {
                self.impulse_real[partition][..end - start].copy_from_slice(&samples[start..end]);
            }
            self.fft.forward(
                &mut self.impulse_real[partition],
                &mut self.impulse_imag[partition],
            );
        }
        self.reset();
        true
    }

    fn convolve_block(&mut self) {
        self.scratch_real.fill(0.0);
        self.scratch_imag.fill(0.0);
        self.scratch_real[..PARTITION_SIZE].copy_from_slice(&self.history);
        self.fft
            .forward(&mut self.scratch_real, &mut self.scratch_imag);
        for partition in (1..MAX_PARTITIONS).rev() {
            self.segments_real[partition] = self.segments_real[partition - 1];
            self.segments_imag[partition] = self.segments_imag[partition - 1];
        }
        self.segments_real[0] = self.scratch_real;
        self.segments_imag[0] = self.scratch_imag;
        self.scratch_real.fill(0.0);
        self.scratch_imag.fill(0.0);
        for partition in 0..MAX_PARTITIONS {
            for bin in 0..FFT_SIZE {
                let ar = self.segments_real[partition][bin];
                let ai = self.segments_imag[partition][bin];
                let br = self.impulse_real[partition][bin];
                let bi = self.impulse_imag[partition][bin];
                self.scratch_real[bin] += ar * br - ai * bi;
                self.scratch_imag[bin] += ar * bi + ai * br;
            }
        }
        self.fft
            .inverse(&mut self.scratch_real, &mut self.scratch_imag);
        self.overlap
            .copy_from_slice(&self.scratch_real[..PARTITION_SIZE]);
    }

    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>, mix: f32) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |right| left.len().min(right.len()));
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut right = right;
        for index in 0..frames {
            let input_left = if left[index].is_finite() {
                left[index]
            } else {
                0.0
            };
            let input_right = right.as_ref().map_or(input_left, |channel| {
                if channel[index].is_finite() {
                    channel[index]
                } else {
                    0.0
                }
            });
            let input = (input_left + input_right) * 0.5;
            self.history[self.write_index] = input;
            let wet = self.overlap[self.write_index] * 0.4;
            let output_left = input_left * (1.0 - mix) + wet * mix;
            left[index] = if output_left.is_finite() {
                output_left
            } else {
                0.0
            };
            if let Some(channel) = right.as_deref_mut() {
                let output_right = input_right * (1.0 - mix) + wet * mix;
                channel[index] = if output_right.is_finite() {
                    output_right
                } else {
                    0.0
                };
            }
            self.write_index += 1;
            if self.write_index == PARTITION_SIZE {
                self.convolve_block();
                self.write_index = 0;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_convolution_reverb_create(sample_rate: f64) -> *mut c_void {
    ConvolutionReverb::new(sample_rate).map_or(std::ptr::null_mut(), |state| {
        Box::into_raw(Box::new(state)).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ConvolutionReverb>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<ConvolutionReverb>().as_mut() } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_tail(state: *const c_void) -> u32 {
    unsafe { state.cast::<ConvolutionReverb>().as_ref() }.map_or(158_760, |state| {
        (3.6 * state.sample_rate as f64).min(u32::MAX as f64) as u32
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<ConvolutionReverb>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_set_ir(state: *mut c_void, model: u32) {
    if let Some(state) = unsafe { state.cast::<ConvolutionReverb>().as_mut() } {
        state.set_ir(model.min(1));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_load_ir(
    state: *mut c_void,
    samples: *const f32,
    sample_count: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ConvolutionReverb>().as_mut() }) else {
        return false;
    };
    if samples.is_null() || sample_count == 0 || sample_count > MAX_PARTITIONS * PARTITION_SIZE {
        return false;
    }
    state.load_ir(unsafe { slice::from_raw_parts(samples, sample_count) })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_convolution_reverb_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<ConvolutionReverb>().as_mut() }) else {
        return;
    };
    if left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    unsafe {
        let left = slice::from_raw_parts_mut(left, frames);
        if left.as_mut_ptr() == right {
            state.process(left, None, mix);
        } else {
            state.process(left, Some(slice::from_raw_parts_mut(right, frames)), mix);
        }
    }
}
