//! Arbitrary-length complex FFT used by the Rust Signalsmith port.
//!
//! The native Signalsmith preset uses a 5,760-sample transform at 48 kHz;
//! Hirari's older `FftPlan` is deliberately radix-2 only. This plan accepts
//! mixed-radix lengths and keeps planner/scratch allocation on construction,
//! so transforms can be used from a prepared realtime state.

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

pub(crate) struct SignalsmithFft {
    size: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
}

struct SignalsmithModifiedRealFft {
    fft: SignalsmithFft,
    size: usize,
    frame: Vec<Complex32>,
    half_bin_shift: Vec<Complex32>,
}

impl SignalsmithModifiedRealFft {
    fn new(size: usize) -> Option<Self> {
        if size < 2 || size % 2 != 0 {
            return None;
        }
        Some(Self {
            fft: SignalsmithFft::new(size)?,
            size,
            frame: vec![Complex32::new(0.0, 0.0); size],
            half_bin_shift: (0..size)
                .map(|index| {
                    Complex32::from_polar(1.0, -std::f32::consts::PI * index as f32 / size as f32)
                })
                .collect(),
        })
    }

    fn forward(&mut self, input: &[f32], real: &mut [f32], imag: &mut [f32]) -> bool {
        let half = self.size / 2;
        if input.len() != self.size || real.len() < half || imag.len() < half {
            return false;
        }
        for index in 0..self.size {
            self.frame[index] = self.half_bin_shift[index] * input[index];
        }
        if !self.fft.forward(&mut self.frame) {
            return false;
        }
        for bin in 0..half {
            real[bin] = self.frame[bin].re;
            imag[bin] = self.frame[bin].im;
        }
        true
    }

    fn inverse(&mut self, real: &[f32], imag: &[f32], output: &mut [f32]) -> bool {
        let half = self.size / 2;
        if real.len() < half || imag.len() < half || output.len() != self.size {
            return false;
        }
        self.frame.fill(Complex32::new(0.0, 0.0));
        for bin in 0..half {
            let value = Complex32::new(real[bin], imag[bin]);
            self.frame[bin] = value;
            self.frame[self.size - 1 - bin] = value.conj();
        }
        if !self.fft.inverse(&mut self.frame) {
            return false;
        }
        let scale = self.size as f32;
        for index in 0..self.size {
            output[index] = (self.frame[index] * self.half_bin_shift[index].conj()).re * scale;
        }
        true
    }
}

#[no_mangle]
pub extern "C" fn hirari_signalsmith_real_fft_create(size: usize) -> *mut std::ffi::c_void {
    SignalsmithModifiedRealFft::new(size).map_or(std::ptr::null_mut(), |plan| {
        Box::into_raw(Box::new(plan)).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signalsmith_real_fft_destroy(plan: *mut std::ffi::c_void) {
    if !plan.is_null() {
        drop(unsafe { Box::from_raw(plan.cast::<SignalsmithModifiedRealFft>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signalsmith_real_fft_forward(
    plan: *mut std::ffi::c_void,
    input: *const f32,
    output_real: *mut f32,
    output_imag: *mut f32,
) -> bool {
    if plan.is_null() || input.is_null() || output_real.is_null() || output_imag.is_null() {
        return false;
    }
    let state = unsafe { &mut *plan.cast::<SignalsmithModifiedRealFft>() };
    let half = state.size / 2;
    unsafe {
        state.forward(
            std::slice::from_raw_parts(input, state.size),
            std::slice::from_raw_parts_mut(output_real, half),
            std::slice::from_raw_parts_mut(output_imag, half),
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signalsmith_real_fft_inverse(
    plan: *mut std::ffi::c_void,
    input_real: *const f32,
    input_imag: *const f32,
    output: *mut f32,
) -> bool {
    if plan.is_null() || input_real.is_null() || input_imag.is_null() || output.is_null() {
        return false;
    }
    let state = unsafe { &mut *plan.cast::<SignalsmithModifiedRealFft>() };
    let half = state.size / 2;
    unsafe {
        state.inverse(
            std::slice::from_raw_parts(input_real, half),
            std::slice::from_raw_parts(input_imag, half),
            std::slice::from_raw_parts_mut(output, state.size),
        )
    }
}

impl SignalsmithFft {
    pub(crate) fn new(size: usize) -> Option<Self> {
        if size < 2 || size > (1 << 20) {
            return None;
        }
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Some(Self {
            size,
            forward,
            inverse,
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
        })
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }

    pub(crate) fn forward(&mut self, spectrum: &mut [Complex32]) -> bool {
        if spectrum.len() != self.size {
            return false;
        }
        self.forward
            .process_with_scratch(spectrum, &mut self.scratch);
        true
    }

    pub(crate) fn inverse(&mut self, spectrum: &mut [Complex32]) -> bool {
        if spectrum.len() != self.size {
            return false;
        }
        self.inverse
            .process_with_scratch(spectrum, &mut self.scratch);
        let scale = 1.0 / self.size as f32;
        for value in spectrum {
            value.re *= scale;
            value.im *= scale;
        }
        true
    }
}

/// Reusable analysis/synthesis frame. The Kaiser window and overlap
/// normalization mirror Signalsmith Linear's STFT setup. `analysis` keeps the
/// positive-frequency bins; `synthesis` rebuilds the Hermitian frame.
pub(crate) struct SignalsmithStftFrame {
    fft: SignalsmithFft,
    block_size: usize,
    interval: usize,
    analysis_offset: usize,
    window: Vec<f32>,
    half_bin_shift: Vec<Complex32>,
    frame: Vec<Complex32>,
    spectrum: Vec<Complex32>,
}

impl SignalsmithStftFrame {
    pub(crate) fn new(block_size: usize, interval: usize) -> Option<Self> {
        if block_size < 2 || interval == 0 || interval > block_size {
            return None;
        }
        let fft_size = signalsmith_real_fft_size(block_size)?;
        Some(Self {
            fft: SignalsmithFft::new(fft_size)?,
            block_size,
            interval,
            analysis_offset: block_size / 2,
            window: kaiser_pr_window(block_size, interval),
            half_bin_shift: (0..fft_size)
                .map(|index| {
                    Complex32::from_polar(
                        1.0,
                        -std::f32::consts::PI * index as f32 / fft_size as f32,
                    )
                })
                .collect(),
            frame: vec![Complex32::new(0.0, 0.0); fft_size],
            spectrum: vec![Complex32::new(0.0, 0.0); fft_size / 2],
        })
    }

    pub(crate) fn block_size(&self) -> usize {
        self.block_size
    }

    pub(crate) fn interval(&self) -> usize {
        // The normalized window is generated for this hop; storing it here
        // also makes the frame's realtime contract explicit.
        self.interval
    }

    pub(crate) fn window(&self) -> &[f32] {
        &self.window
    }

    pub(crate) fn analysis(&mut self, input: &[f32]) -> &[Complex32] {
        self.frame.fill(Complex32::new(0.0, 0.0));
        for index in 0..self.block_size {
            let position = if index < self.analysis_offset {
                self.frame.len() - self.analysis_offset + index
            } else {
                index - self.analysis_offset
            };
            let sign = if index < self.analysis_offset {
                -1.0
            } else {
                1.0
            };
            self.frame[position] = self.half_bin_shift[position]
                * (input.get(index).copied().unwrap_or(0.0) * self.window[index] * sign);
        }
        self.fft.forward(&mut self.frame);
        let half = self.frame.len() / 2;
        self.spectrum.copy_from_slice(&self.frame[..half]);
        &self.spectrum
    }

    pub(crate) fn synthesis(&mut self, spectrum: &[Complex32], output: &mut [f32]) -> bool {
        if spectrum.len() != self.spectrum.len() || output.len() != self.block_size {
            return false;
        }
        self.frame[..spectrum.len()].copy_from_slice(spectrum);
        let size = self.frame.len();
        let half = size / 2;
        for bin in 0..half {
            self.frame[size - 1 - bin] = spectrum[bin].conj();
        }
        self.fft.inverse(&mut self.frame);
        for index in 0..self.block_size {
            let position = if index < self.analysis_offset {
                self.frame.len() - self.analysis_offset + index
            } else {
                index - self.analysis_offset
            };
            let sign = if index < self.analysis_offset {
                -1.0
            } else {
                1.0
            };
            output[index] = (self.frame[position] * self.half_bin_shift[position].conj()).re
                * self.window[index]
                * sign;
        }
        true
    }
}

fn signalsmith_real_fft_size(block_size: usize) -> Option<usize> {
    let target = block_size.checked_add(1)?.checked_div(2)?;
    let mut power_of_two = 1usize;
    while power_of_two < 16 && power_of_two < target {
        power_of_two = power_of_two.checked_mul(2)?;
    }
    while power_of_two.checked_mul(8)? < target {
        power_of_two = power_of_two.checked_mul(2)?;
    }
    let mut multiple = target.checked_add(power_of_two - 1)? / power_of_two;
    if multiple == 7 {
        multiple += 1;
    }
    multiple.checked_mul(power_of_two)?.checked_mul(2)
}

fn kaiser_pr_window(size: usize, interval: usize) -> Vec<f32> {
    let bandwidth = size as f64 / interval as f64;
    let bandwidth = bandwidth
        + 8.0 / ((bandwidth + 3.0) * (bandwidth + 3.0))
        + 0.25 * (3.0 - bandwidth).max(0.0);
    let bandwidth = bandwidth.max(2.0);
    let alpha = (bandwidth * bandwidth * 0.25 - 1.0).sqrt();
    let beta = alpha * std::f64::consts::PI;
    let inv_b0 = 1.0 / bessel0(beta);
    let mut window = (0..size)
        .map(|index| {
            let mut r = (2 * index) as f64 / size as f64 - 1.0;
            // Signalsmith's synthesis Kaiser window uses the even-size
            // synthesis offset (zero), then applies perfect-reconstruction
            // scaling over every hop residue class.
            let arg = (1.0 - r * r).max(0.0).sqrt();
            r = bessel0(beta * arg) * inv_b0;
            r as f32
        })
        .collect::<Vec<_>>();
    for residue in 0..interval {
        let energy = (residue..size)
            .step_by(interval)
            .map(|index| (window[index] * window[index]) as f64)
            .sum::<f64>();
        if energy > 0.0 {
            let scale = energy.sqrt().recip();
            for index in (residue..size).step_by(interval) {
                window[index] = (window[index] as f64 * scale) as f32;
            }
        }
    }
    window
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signalsmith_kaiser_pr_window(
    size: usize,
    interval: usize,
    output: *mut f32,
) -> bool {
    if output.is_null()
        || size < 2
        || size % 2 != 0
        || size > (1 << 20)
        || interval == 0
        || interval > size
    {
        return false;
    }
    let window = kaiser_pr_window(size, interval);
    unsafe { std::slice::from_raw_parts_mut(output, size).copy_from_slice(&window) };
    true
}

fn bessel0(value: f64) -> f64 {
    let mut result = 0.0;
    let mut term = 1.0;
    let mut order = 0.0;
    while term > 1.0e-4 {
        result += term;
        order += 1.0;
        term *= (value * value) / (4.0 * order * order);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_length_transform_round_trips_without_allocating_per_call() {
        for size in [2, 12, 2048, 5760, 8192] {
            let Some(mut fft) = SignalsmithFft::new(size) else {
                panic!("plan rejected supported size {size}");
            };
            assert_eq!(fft.size(), size);
            let input = (0..size)
                .map(|i| {
                    let x = i as f32;
                    Complex32::new((x * 0.013).sin(), (x * 0.021).cos() * 0.25)
                })
                .collect::<Vec<_>>();
            let mut spectrum = input.clone();
            assert!(fft.forward(&mut spectrum));
            assert!(fft.inverse(&mut spectrum));
            for (index, (actual, expected)) in spectrum.iter().zip(input).enumerate() {
                assert!(
                    (actual.re - expected.re).abs() < 2.0e-4
                        && (actual.im - expected.im).abs() < 2.0e-4,
                    "size={size} sample={index} actual={actual:?} expected={expected:?}"
                );
            }
        }
    }

    #[test]
    fn impulse_has_unit_magnitude_in_every_bin_for_non_power_of_two_size() {
        let size = 5760;
        let mut fft = SignalsmithFft::new(size).unwrap();
        let mut values = vec![Complex32::new(0.0, 0.0); size];
        values[0].re = 1.0;
        assert!(fft.forward(&mut values));
        assert!(values
            .iter()
            .all(|value| { (value.re - 1.0).abs() < 1.0e-6 && value.im.abs() < 1.0e-6 }));
    }

    #[test]
    fn native_size_kaiser_window_is_perfect_reconstruction_at_each_hop_phase() {
        let block = 5760;
        let interval = 1440;
        let frame = SignalsmithStftFrame::new(block, interval).unwrap();
        assert_eq!(frame.block_size(), block);
        assert_eq!(frame.interval(), interval);
        for residue in 0..interval {
            let overlap_power = (residue..block)
                .step_by(interval)
                .map(|index| frame.window()[index] * frame.window()[index])
                .sum::<f32>();
            assert!(
                (overlap_power - 1.0).abs() < 2.0e-6,
                "phase={residue} power={overlap_power}"
            );
        }
    }

    #[test]
    fn native_size_analysis_synthesis_frame_round_trips_in_the_overlap_region() {
        let block = 5760;
        let interval = 1440;
        let mut frame = SignalsmithStftFrame::new(block, interval).unwrap();
        let input = (0..block + interval * 6)
            .map(|index| {
                let x = index as f32;
                (x * 0.013).sin() * 0.6 + (x * 0.0017).cos() * 0.2
            })
            .collect::<Vec<_>>();
        let mut output = vec![0.0f32; input.len()];
        for start in (0..input.len() - block).step_by(interval) {
            let spectrum = frame.analysis(&input[start..start + block]).to_vec();
            let mut synthesized = vec![0.0f32; block];
            assert!(frame.synthesis(&spectrum, &mut synthesized));
            for index in 0..block {
                output[start + index] += synthesized[index];
            }
        }
        for index in block..input.len() - block {
            assert!(
                (output[index] - input[index]).abs() < 3.0e-5,
                "sample={index} actual={} expected={}",
                output[index],
                input[index]
            );
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_modified_real_fft_matches_signalsmith_cpp_forward_and_inverse() {
        unsafe extern "C" {
            fn hirari_signalsmith_real_fft_forward_reference(
                input: *const f32,
                size: usize,
                output_real: *mut f32,
                output_imag: *mut f32,
            ) -> bool;
            fn hirari_signalsmith_real_fft_inverse_reference(
                input_real: *const f32,
                input_imag: *const f32,
                size: usize,
                output: *mut f32,
            ) -> bool;
        }

        for size in [2048, 6144] {
            let input = (0..size)
                .map(|index| {
                    let x = index as f32;
                    (x * 0.013).sin() * 0.6 + (x * 0.0017).cos() * 0.2 + (x * 0.19).sin() * 0.05
                })
                .collect::<Vec<_>>();
            let half = size / 2;
            let mut rust_real = vec![0.0f32; half];
            let mut rust_imag = vec![0.0f32; half];
            let mut cpp_real = vec![0.0f32; half];
            let mut cpp_imag = vec![0.0f32; half];
            let plan = hirari_signalsmith_real_fft_create(size);
            assert!(!plan.is_null());
            assert!(unsafe {
                hirari_signalsmith_real_fft_forward(
                    plan,
                    input.as_ptr(),
                    rust_real.as_mut_ptr(),
                    rust_imag.as_mut_ptr(),
                )
            });
            assert!(unsafe {
                hirari_signalsmith_real_fft_forward_reference(
                    input.as_ptr(),
                    size,
                    cpp_real.as_mut_ptr(),
                    cpp_imag.as_mut_ptr(),
                )
            });
            for bin in 0..half {
                let tolerance = 3.0e-4 * cpp_real[bin].abs().max(cpp_imag[bin].abs()).max(1.0);
                assert!(
                    (rust_real[bin] - cpp_real[bin]).abs() <= tolerance
                        && (rust_imag[bin] - cpp_imag[bin]).abs() <= tolerance,
                    "size={size} bin={bin} Rust=({}, {}) C++=({}, {}) tolerance={tolerance}",
                    rust_real[bin],
                    rust_imag[bin],
                    cpp_real[bin],
                    cpp_imag[bin]
                );
            }

            let mut rust_time = vec![0.0f32; size];
            let mut cpp_time = vec![0.0f32; size];
            assert!(unsafe {
                hirari_signalsmith_real_fft_inverse(
                    plan,
                    cpp_real.as_ptr(),
                    cpp_imag.as_ptr(),
                    rust_time.as_mut_ptr(),
                )
            });
            assert!(unsafe {
                hirari_signalsmith_real_fft_inverse_reference(
                    cpp_real.as_ptr(),
                    cpp_imag.as_ptr(),
                    size,
                    cpp_time.as_mut_ptr(),
                )
            });
            for index in 0..size {
                // RustFFT and Signalsmith's radix/mixed-radix kernels sum in
                // different orders; bound their single-precision inverse
                // drift while keeping the tolerance below one tenth percent.
                let tolerance = 1.0e-3 * cpp_time[index].abs().max(1.0);
                assert!(
                    (rust_time[index] - cpp_time[index]).abs() <= tolerance,
                    "inverse size={size} sample={index} Rust={} C++={} tolerance={tolerance}",
                    rust_time[index],
                    cpp_time[index]
                );
            }
            unsafe { hirari_signalsmith_real_fft_destroy(plan) };
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_kaiser_pr_window_matches_signalsmith_cpp_window() {
        unsafe extern "C" {
            fn hirari_signalsmith_kaiser_window_reference(
                size: usize,
                interval: usize,
                output: *mut f32,
            ) -> bool;
        }

        for (size, interval) in [(2048, 128), (5760, 1440), (8192, 512)] {
            let mut rust = vec![0.0f32; size];
            let mut cpp = vec![0.0f32; size];
            assert!(unsafe {
                hirari_signalsmith_kaiser_pr_window(size, interval, rust.as_mut_ptr())
            });
            assert!(unsafe {
                hirari_signalsmith_kaiser_window_reference(size, interval, cpp.as_mut_ptr())
            });
            for (index, (actual, expected)) in rust.iter().zip(cpp).enumerate() {
                assert!(
                    (actual - expected).abs() < 2.0e-6,
                    "size={size} interval={interval} index={index} Rust={actual} C++={expected}"
                );
            }
        }
        let mut odd = [0.0f32; 3];
        assert!(!unsafe { hirari_signalsmith_kaiser_pr_window(3, 1, odd.as_mut_ptr()) });
    }
}
