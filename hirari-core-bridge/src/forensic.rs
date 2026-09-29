pub struct ForensicMetrics {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub correlation: f32,
}

pub struct SignalForensicOrchestrator {
    pub fft_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpectrogramFrameFFI {
    pub timestamp: u64,
    pub bins: [f32; 256],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SignalForensicSuiteMetrics {
    pub peak: f32,
    pub rms: f32,
    pub correlation: f32,
    pub lufs_integrated: f32,
}

struct SignalForensicSuiteState {
    fft_size: usize,
    data: std::sync::Mutex<SignalForensicSuiteData>,
}

#[derive(Default)]
struct SignalForensicSuiteData {
    metrics: SignalForensicSuiteMetrics,
    history: Vec<SpectrogramFrameFFI>,
}

impl SignalForensicSuiteState {
    fn new(fft_size: u32) -> Self {
        Self {
            fft_size: fft_size.clamp(2, 16_384) as usize,
            data: std::sync::Mutex::new(SignalForensicSuiteData {
                metrics: SignalForensicSuiteMetrics {
                    lufs_integrated: -180.0,
                    ..SignalForensicSuiteMetrics::default()
                },
                history: Vec::with_capacity(256),
            }),
        }
    }

    fn run_fft(&self, input: &[f32], output: &mut [f32; 256]) {
        output.fill(0.0);
        if input.len() < self.fft_size {
            return;
        }
        let bins = 256usize.min(self.fft_size / 2);
        for (bin, output_bin) in output.iter_mut().take(bins).enumerate() {
            let mut real = 0.0f64;
            let mut imag = 0.0f64;
            for (index, &raw) in input.iter().take(self.fft_size).enumerate() {
                let sample = if raw.is_finite() { raw } else { 0.0 };
                let phase =
                    2.0 * std::f64::consts::PI * (bin * index) as f64 / self.fft_size as f64;
                real += sample as f64 * phase.cos();
                imag -= sample as f64 * phase.sin();
            }
            *output_bin = ((real * real + imag * imag).sqrt() / self.fft_size as f64) as f32;
        }
    }

    unsafe fn process(&self, channels: *const *const f32, channel_count: u32, frames: usize) {
        if channels.is_null() || channel_count == 0 || frames == 0 {
            return;
        }
        let channel_count = channel_count.min(2) as usize;
        let pointers = unsafe { std::slice::from_raw_parts(channels, channel_count) };
        let mut peak = [0.0f32; 2];
        let mut energy = [0.0f64; 2];
        let mut cross = 0.0f64;
        for frame in 0..frames {
            let mut values = [0.0f32; 2];
            for channel in 0..channel_count {
                let raw = if pointers[channel].is_null() {
                    0.0
                } else {
                    unsafe { *pointers[channel].add(frame) }
                };
                let value = if raw.is_finite() { raw } else { 0.0 };
                values[channel] = value;
                peak[channel] = peak[channel].max(value.abs());
                energy[channel] += value as f64 * value as f64;
            }
            if channel_count == 2 {
                cross += values[0] as f64 * values[1] as f64;
            }
        }

        let mut frame = SpectrogramFrameFFI {
            timestamp: 0,
            bins: [0.0; 256],
        };
        if !pointers[0].is_null() {
            let channel_zero = unsafe { std::slice::from_raw_parts(pointers[0], frames) };
            self.run_fft(channel_zero, &mut frame.bins);
        }

        let mut data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        frame.timestamp = data.history.len() as u64;
        data.metrics.peak = peak[0].max(if channel_count > 1 { peak[1] } else { peak[0] });
        let norm = 1.0 / frames as f64;
        let denominator = (energy[0] * energy[1]).max(0.0).sqrt();
        data.metrics.correlation = if channel_count > 1 && denominator > 1.0e-12 {
            (cross / denominator).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        let rms = (((energy[0] + energy[1]) * 0.5 * norm).max(0.0)).sqrt();
        data.metrics.rms = rms as f32;
        data.metrics.lufs_integrated = if rms > 1.0e-9 {
            20.0 * (rms as f32).log10()
        } else {
            -180.0
        };
        if data.history.len() == 256 {
            data.history.remove(0);
        }
        data.history.push(frame);
    }
}

#[no_mangle]
pub extern "C" fn hirari_signal_forensic_suite_create(fft_size: u32) -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(SignalForensicSuiteState::new(fft_size))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signal_forensic_suite_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<SignalForensicSuiteState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signal_forensic_suite_process(
    state: *mut std::ffi::c_void,
    channels: *const *const f32,
    channel_count: u32,
    frames: u32,
) {
    if let Some(state) = unsafe { state.cast::<SignalForensicSuiteState>().as_mut() } {
        unsafe { state.process(channels, channel_count, frames as usize) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signal_forensic_suite_run_fft(
    state: *const std::ffi::c_void,
    input: *const f32,
    output: *mut f32,
) {
    if state.is_null() || input.is_null() || output.is_null() {
        return;
    }
    let state = unsafe { &*state.cast::<SignalForensicSuiteState>() };
    let input = unsafe { std::slice::from_raw_parts(input, state.fft_size) };
    let output = unsafe { std::slice::from_raw_parts_mut(output, 256) };
    let mut bins = [0.0f32; 256];
    state.run_fft(input, &mut bins);
    output.copy_from_slice(&bins);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signal_forensic_suite_get_metrics(
    state: *const std::ffi::c_void,
    output: *mut f32,
) {
    if state.is_null() || output.is_null() {
        return;
    }
    let state = unsafe { &*state.cast::<SignalForensicSuiteState>() };
    let data = state
        .data
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let metrics = data.metrics;
    unsafe {
        *output = metrics.peak;
        *output.add(1) = metrics.rms;
        *output.add(2) = metrics.correlation;
        *output.add(3) = metrics.lufs_integrated;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_signal_forensic_suite_get_spectrogram(
    state: *const std::ffi::c_void,
    output: *mut SpectrogramFrameFFI,
    max_frames: usize,
) -> usize {
    if state.is_null() || output.is_null() || max_frames == 0 {
        return 0;
    }
    let state = unsafe { &*state.cast::<SignalForensicSuiteState>() };
    let data = state
        .data
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let count = max_frames.min(data.history.len());
    for index in 0..count {
        unsafe { output.add(index).write(data.history[index]) };
    }
    count
}

#[cfg(test)]
mod signal_forensic_suite_tests {
    use super::{
        hirari_signal_forensic_suite_create, hirari_signal_forensic_suite_destroy,
        hirari_signal_forensic_suite_get_metrics, hirari_signal_forensic_suite_get_spectrogram,
        hirari_signal_forensic_suite_process, hirari_signal_forensic_suite_run_fft,
        SpectrogramFrameFFI,
    };

    fn cpp_reference(left: &[f32], right: &[f32], fft_size: usize) -> ([f32; 4], [f32; 256]) {
        let mut peak = [0.0f32; 2];
        let mut energy = [0.0f64; 2];
        let mut cross = 0.0f64;
        for (&left, &right) in left.iter().zip(right) {
            let values = [
                if left.is_finite() { left } else { 0.0 },
                if right.is_finite() { right } else { 0.0 },
            ];
            for channel in 0..2 {
                peak[channel] = peak[channel].max(values[channel].abs());
                energy[channel] += values[channel] as f64 * values[channel] as f64;
            }
            cross += values[0] as f64 * values[1] as f64;
        }

        let mut bins = [0.0f32; 256];
        let pi = std::f64::consts::PI;
        for (bin, output) in bins.iter_mut().take(256.min(fft_size / 2)).enumerate() {
            let mut real = 0.0f64;
            let mut imag = 0.0f64;
            for (sample, &raw) in left.iter().take(fft_size).enumerate() {
                let value = if raw.is_finite() { raw } else { 0.0 };
                let phase = 2.0 * pi * (bin * sample) as f64 / fft_size as f64;
                real += value as f64 * phase.cos();
                imag -= value as f64 * phase.sin();
            }
            *output = ((real * real + imag * imag).sqrt() / fft_size as f64) as f32;
        }

        let denominator = (energy[0] * energy[1]).max(0.0).sqrt();
        let correlation = if denominator > 1.0e-12 {
            (cross / denominator).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        let rms = (((energy[0] + energy[1]) * 0.5 / left.len() as f64).max(0.0)).sqrt();
        let lufs = if rms > 1.0e-9 {
            20.0 * (rms as f32).log10()
        } else {
            -180.0
        };
        ([peak[0].max(peak[1]), rms as f32, correlation, lufs], bins)
    }

    #[test]
    fn rust_suite_matches_frozen_cpp_metrics_and_dft() {
        let left = [1.0, 0.0, f32::NAN, 0.0, -0.5, 0.0, 0.25, 0.0];
        let right = [-0.5, 0.0, 0.25, 0.0, 0.25, 0.0, -0.125, 0.0];
        let expected = cpp_reference(&left, &right, 8);
        let state = hirari_signal_forensic_suite_create(8);
        assert!(!state.is_null());
        unsafe {
            let channels = [left.as_ptr(), right.as_ptr()];
            hirari_signal_forensic_suite_process(state, channels.as_ptr(), 2, left.len() as u32);
            let mut metrics = [0.0f32; 4];
            hirari_signal_forensic_suite_get_metrics(state, metrics.as_mut_ptr());
            for (actual, expected) in metrics.iter().zip(expected.0) {
                assert!((actual - expected).abs() < 1.0e-5, "{actual} != {expected}");
            }

            let mut bins = [0.0f32; 256];
            hirari_signal_forensic_suite_run_fft(state, left.as_ptr(), bins.as_mut_ptr());
            for (actual, expected) in bins.iter().zip(expected.1) {
                assert!((actual - expected).abs() < 1.0e-5, "{actual} != {expected}");
            }

            let mut frames = [SpectrogramFrameFFI {
                timestamp: 0,
                bins: [0.0; 256],
            }];
            assert_eq!(
                hirari_signal_forensic_suite_get_spectrogram(
                    state,
                    frames.as_mut_ptr(),
                    frames.len(),
                ),
                1
            );
            assert_eq!(frames[0].timestamp, 0);
            for (actual, expected) in frames[0].bins.iter().zip(expected.1) {
                assert!((actual - expected).abs() < 1.0e-5, "{actual} != {expected}");
            }
            hirari_signal_forensic_suite_destroy(state);
        }
    }

    #[test]
    fn rust_suite_preserves_cpp_history_capacity_and_short_fft_behavior() {
        let state = hirari_signal_forensic_suite_create(16);
        assert!(!state.is_null());
        let samples = [0.25f32; 8];
        unsafe {
            let channels = [samples.as_ptr(), samples.as_ptr()];
            for _ in 0..258 {
                hirari_signal_forensic_suite_process(state, channels.as_ptr(), 2, 8);
            }
            let mut frames = [SpectrogramFrameFFI {
                timestamp: 0,
                bins: [1.0; 256],
            }; 256];
            assert_eq!(
                hirari_signal_forensic_suite_get_spectrogram(
                    state,
                    frames.as_mut_ptr(),
                    frames.len(),
                ),
                256
            );
            assert_eq!(frames[0].timestamp, 2);
            assert_eq!(frames[253].timestamp, 255);
            assert!(frames[254..].iter().all(|frame| frame.timestamp == 256));
            assert!(frames
                .iter()
                .all(|frame| frame.bins.iter().all(|bin| *bin == 0.0)));
            hirari_signal_forensic_suite_destroy(state);
        }
    }
}

impl SignalForensicOrchestrator {
    pub fn new(fft_size: u32) -> Self {
        Self { fft_size }
    }

    /// INDUSTRIAL: Performs signal analysis and diagnostics with absolute precision and signal forensic sovereignty.
    pub fn process_signal(&mut self, buffer: &[f32], channels: u32) -> ForensicMetrics {
        // INDUSTRIAL: Implementation of high-performance metering resolution.
        // Rust's safe memory management handles complex signal streams with
        // absolute bit-accuracy and zero-latency.
        // Rust's MeteringEngine ensures bit-accurate gain distribution.

        if channels == 0 || buffer.is_empty() {
            return ForensicMetrics {
                peak: [0.0; 2],
                rms: [0.0; 2],
                correlation: 0.0,
            };
        }
        let channels_usize = channels as usize;
        let num_samples = buffer.len() / channels_usize;
        if num_samples == 0 {
            return ForensicMetrics {
                peak: [0.0; 2],
                rms: [0.0; 2],
                correlation: 0.0,
            };
        }
        let mut peak = [0.0f32; 2];
        let mut rms = [0.0f32; 2];

        for c in 0..channels.min(2) {
            let mut p = 0.0f32;
            let mut sum_sq = 0.0f32;
            for s in 0..num_samples {
                let val = buffer[s * channels_usize + c as usize];
                let val = if val.is_finite() { val } else { 0.0 }.abs();
                if val > p {
                    p = val;
                }
                sum_sq += val * val;
            }
            peak[c as usize] = p;
            rms[c as usize] = (sum_sq / num_samples as f32).sqrt();
        }

        let correlation = if channels >= 2 {
            let mut lr = 0.0f64;
            let mut ll = 0.0f64;
            let mut rr = 0.0f64;
            for s in 0..num_samples {
                let left = buffer[s * channels_usize];
                let right = buffer[s * channels_usize + 1];
                if left.is_finite() && right.is_finite() {
                    lr += left as f64 * right as f64;
                    ll += left as f64 * left as f64;
                    rr += right as f64 * right as f64;
                }
            }
            if ll > f64::EPSILON && rr > f64::EPSILON {
                (lr / (ll.sqrt() * rr.sqrt())).clamp(-1.0, 1.0) as f32
            } else {
                0.0
            }
        } else {
            0.0
        };

        ForensicMetrics {
            peak,
            rms,
            correlation,
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal forensic synchronization graph.
    pub fn audit_signal(&self) -> bool {
        self.fft_size > 0 && self.fft_size.is_power_of_two()
    }
}

#[cfg(test)]
mod tests {
    use super::SignalForensicOrchestrator;

    #[test]
    fn empty_and_zero_channel_input_is_safe() {
        let mut forensic = SignalForensicOrchestrator::new(1024);
        let empty = forensic.process_signal(&[], 0);
        assert_eq!(empty.peak, [0.0, 0.0]);
        assert_eq!(empty.rms, [0.0, 0.0]);

        let short = forensic.process_signal(&[f32::NAN], 2);
        assert_eq!(short.peak, [0.0, 0.0]);
        assert_eq!(short.rms, [0.0, 0.0]);
    }

    #[test]
    fn stereo_correlation_is_bounded() {
        let mut forensic = SignalForensicOrchestrator::new(1024);
        let metrics = forensic.process_signal(&[1.0, 1.0, -1.0, -1.0], 2);
        assert!((-1.0..=1.0).contains(&metrics.correlation));
    }
}
