use std::ptr;

use crate::signalsmith_stretch::SignalsmithStretch;

/// Native Rust port of the Signalsmith stretch engine (`signalsmith-stretch.h`).
/// The former C++ `stretch_kernel.cpp` shim is replaced by this wrapper.
struct StretchState {
    kernel: SignalsmithStretch,
    input_left: Vec<f32>,
    input_right: Vec<f32>,
    output_left: Vec<f32>,
    output_right: Vec<f32>,
    seek_left: Vec<f32>,
    seek_right: Vec<f32>,
    sample_rate: f64,
    max_block_size: usize,
    input_capacity: usize,
    next_timeline_offset: u64,
    next_input_offset: u64,
    source_offset: u64,
    source_span: u64,
    source_left: *const f32,
    source_right: *const f32,
    reverse: bool,
    stream_valid: bool,
    prepared: bool,
}

impl StretchState {
    fn new() -> Self {
        Self::with_kernel(SignalsmithStretch::new())
    }

    fn with_kernel(kernel: SignalsmithStretch) -> Self {
        Self {
            kernel,
            input_left: Vec::new(),
            input_right: Vec::new(),
            output_left: Vec::new(),
            output_right: Vec::new(),
            seek_left: Vec::new(),
            seek_right: Vec::new(),
            sample_rate: 0.0,
            max_block_size: 0,
            input_capacity: 0,
            next_timeline_offset: 0,
            next_input_offset: 0,
            source_offset: 0,
            source_span: 0,
            source_left: ptr::null(),
            source_right: ptr::null(),
            reverse: false,
            stream_valid: false,
            prepared: false,
        }
    }

    #[cfg(test)]
    fn new_seeded(seed: std::ffi::c_long) -> Self {
        Self::with_kernel(SignalsmithStretch::with_seed(seed as u32))
    }

    fn reset(&mut self) {
        self.kernel.reset();
        self.stream_valid = false;
        self.next_timeline_offset = 0;
        self.next_input_offset = 0;
        self.source_left = ptr::null();
        self.source_right = ptr::null();
        self.source_offset = 0;
        self.source_span = 0;
        self.reverse = false;
    }

    fn prepare(&mut self, sample_rate: f64, max_block_size: u32) -> bool {
        if !sample_rate.is_finite() || sample_rate < 8000.0 || max_block_size == 0 {
            return false;
        }
        if self.prepared
            && self.sample_rate == sample_rate
            && self.max_block_size == max_block_size as usize
        {
            return true;
        }
        self.kernel.preset_default(2, sample_rate as f32, true);
        self.sample_rate = sample_rate;
        self.max_block_size = max_block_size as usize;
        self.input_capacity = self.max_block_size.saturating_mul(4).saturating_add(2);
        if self.input_capacity > i32::MAX as usize || self.max_block_size > i32::MAX as usize {
            return false;
        }
        self.input_left.resize(self.input_capacity, 0.0);
        self.input_right.resize(self.input_capacity, 0.0);
        self.output_left.resize(self.max_block_size, 0.0);
        self.output_right.resize(self.max_block_size, 0.0);
        let seek_len = self
            .kernel
            .output_seek_length(4.0)
            .max(self.kernel.output_seek_length(0.25));
        if seek_len <= 0 {
            return false;
        }
        self.seek_left.resize(seek_len as usize, 0.0);
        self.seek_right.resize(seek_len as usize, 0.0);
        self.prepared = true;
        self.reset();
        true
    }

    unsafe fn render(
        &mut self,
        source_left: *const f32,
        source_right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        timeline_offset: u64,
        source_start: f64,
        source_end: f64,
        output_offset: u32,
        frames: u32,
        reverse: bool,
        transpose: f32,
        formant: f32,
        formant_base_hz: f32,
    ) -> bool {
        if !self.prepared
            || source_left.is_null()
            || source_right.is_null()
            || source_span == 0
            || frames == 0
            || frames as usize > self.max_block_size
            || output_offset as usize > self.max_block_size
            || frames as usize > self.max_block_size - output_offset as usize
            || !source_start.is_finite()
            || !source_end.is_finite()
            || source_start < 0.0
            || source_end <= source_start
            || source_end > source_span as f64
            || !transpose.is_finite()
            || !formant.is_finite()
        {
            return false;
        }
        let formant_base = if formant_base_hz.is_finite() && formant_base_hz > 0.0 {
            formant_base_hz / self.sample_rate as f32
        } else {
            0.0
        };
        self.kernel.set_transpose_semitones(transpose, 0.0);
        self.kernel.set_formant_semitones(formant, true);
        if formant_base > 0.0 {
            self.kernel.set_formant_base(formant_base);
        }
        let input_start_f = source_start.round();
        let input_end_f = source_end.round();
        if input_end_f > u64::MAX as f64 || input_start_f > u64::MAX as f64 {
            return false;
        }
        let input_start = input_start_f as u64;
        let input_end = input_end_f as u64;
        if input_end <= input_start || input_end - input_start > self.input_capacity as u64 {
            return false;
        }
        let input_count = (input_end - input_start) as usize;
        let source_rate = input_count as f64 / frames as f64;
        if !source_rate.is_finite() || !(0.25..=4.0).contains(&source_rate) {
            return false;
        }
        let can_continue = self.stream_valid
            && self.next_timeline_offset == timeline_offset
            && self.next_input_offset == input_start
            && self.source_left == source_left
            && self.source_right == source_right
            && self.source_offset == source_offset
            && self.source_span == source_span
            && self.reverse == reverse;
        if !can_continue
            && !self.seek(
                source_left,
                source_right,
                source_samples,
                source_offset,
                source_span,
                input_start,
                source_rate,
                reverse,
            )
        {
            return false;
        }
        for i in 0..input_count {
            let relative = input_start + i as u64;
            self.input_left[i] = read_source(
                source_left,
                source_samples,
                source_offset,
                source_span,
                relative,
                reverse,
            );
            self.input_right[i] = read_source(
                source_right,
                source_samples,
                source_offset,
                source_span,
                relative,
                reverse,
            );
        }
        let out_start = output_offset as usize;
        let inputs: [&[f32]; 2] = [&self.input_left[..input_count], &self.input_right[..input_count]];
        let (out_l, out_r) = (&mut self.output_left, &mut self.output_right);
        let mut outputs: [&mut [f32]; 2] = [
            &mut out_l[out_start..out_start + frames as usize],
            &mut out_r[out_start..out_start + frames as usize],
        ];
        self.kernel
            .process(&inputs, input_count, &mut outputs, frames as usize);
        self.next_timeline_offset = timeline_offset.saturating_add(frames as u64);
        self.next_input_offset = input_end;
        self.source_left = source_left;
        self.source_right = source_right;
        self.source_offset = source_offset;
        self.source_span = source_span;
        self.reverse = reverse;
        self.stream_valid = true;
        true
    }

    unsafe fn seek(
        &mut self,
        source_left: *const f32,
        source_right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        input_position: u64,
        source_rate: f64,
        reverse: bool,
    ) -> bool {
        let seek_samples = self.kernel.output_seek_length(source_rate as f32);
        if seek_samples <= 0 || seek_samples as usize > self.seek_left.len() {
            return false;
        }
        let begin = input_position as i128 - seek_samples as i128;
        for i in 0..seek_samples as usize {
            let relative = begin + i as i128;
            if relative < 0 {
                self.seek_left[i] = 0.0;
                self.seek_right[i] = 0.0;
            } else {
                self.seek_left[i] = read_source(
                    source_left,
                    source_samples,
                    source_offset,
                    source_span,
                    relative as u64,
                    reverse,
                );
                self.seek_right[i] = read_source(
                    source_right,
                    source_samples,
                    source_offset,
                    source_span,
                    relative as u64,
                    reverse,
                );
            }
        }
        let seek_inputs: [&[f32]; 2] = [&self.seek_left[..seek_samples as usize], &self.seek_right[..seek_samples as usize]];
        self.kernel.output_seek(&seek_inputs, seek_samples as usize);
        self.next_input_offset = input_position;
        self.stream_valid = true;
        true
    }
}

fn read_source(
    source: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    relative: u64,
    reverse: bool,
) -> f32 {
    if relative >= source_span {
        return 0.0;
    }
    let index = if reverse {
        source_offset.checked_add(source_span - 1 - relative)
    } else {
        source_offset.checked_add(relative)
    };
    let Some(index) = index else { return 0.0 };
    if index >= source_samples {
        return 0.0;
    }
    let sample = unsafe { *source.add(index as usize) };
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_region_stretch_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(StretchState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_destroy(handle: *mut std::ffi::c_void) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle.cast::<StretchState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_prepare(
    handle: *mut std::ffi::c_void,
    sample_rate: f64,
    max_block_size: u32,
) -> bool {
    unsafe { handle.cast::<StretchState>().as_mut() }
        .is_some_and(|state| state.prepare(sample_rate, max_block_size))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_render(
    handle: *mut std::ffi::c_void,
    source_left: *const f32,
    source_right: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    timeline_offset: u64,
    source_start: f64,
    source_end: f64,
    output_offset: u32,
    frames: u32,
    reverse: bool,
    transpose: f32,
    formant: f32,
    formant_base_hz: f32,
) -> bool {
    let Some(state) = (unsafe { handle.cast::<StretchState>().as_mut() }) else {
        return false;
    };
    unsafe {
        state.render(
            source_left,
            source_right,
            source_samples,
            source_offset,
            source_span,
            timeline_offset,
            source_start,
            source_end,
            output_offset,
            frames,
            reverse,
            transpose,
            formant,
            formant_base_hz,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_output_left(
    handle: *const std::ffi::c_void,
) -> *const f32 {
    unsafe { handle.cast::<StretchState>().as_ref() }
        .map_or(ptr::null(), |state| state.output_left.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_output_right(
    handle: *const std::ffi::c_void,
) -> *const f32 {
    unsafe { handle.cast::<StretchState>().as_ref() }
        .map_or(ptr::null(), |state| state.output_right.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_lookahead(handle: *const std::ffi::c_void) -> u32 {
    let Some(state) = (unsafe { handle.cast::<StretchState>().as_ref() }) else {
        return 0;
    };
    state.kernel.output_latency().max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_rejects_invalid_rates_and_blocks() {
        let mut state = StretchState::new();
        assert!(!state.prepare(f64::NAN, 512));
        assert!(!state.prepare(48_000.0, 0));
        assert!(state.prepare(48_000.0, 512));
        assert_eq!(state.output_left.len(), 512);
        assert_eq!(state.output_right.len(), 512);
    }

    #[test]
    fn render_produces_finite_output_and_reset_breaks_continuity() {
        let mut state = StretchState::new();
        assert!(state.prepare(48_000.0, 512));
        let left = (0..4096)
            .map(|i| (i as f32 * 0.017).sin())
            .collect::<Vec<_>>();
        let right = left.clone();
        let rendered = unsafe {
            state.render(
                left.as_ptr(),
                right.as_ptr(),
                left.len() as u64,
                0,
                left.len() as u64,
                0,
                0.0,
                256.0,
                0,
                256,
                false,
                0.0,
                0.0,
                1000.0,
            )
        };
        assert!(rendered);
        assert!(state.output_left.iter().all(|sample| sample.is_finite()));
        assert!(state.stream_valid);
        state.reset();
        assert!(!state.stream_valid);
    }
}

#[cfg(test)]
mod differential_tests {
    use super::*;

    #[test]
    fn rust_half_bin_stft_analysis_matches_frozen_cpp_reference() {
        let block = 5760;
        let input = (0..block)
            .map(|index| {
                let x = index as f32;
                (x * 0.013).sin() * 0.6 + (x * 0.0017).cos() * 0.2
            })
            .collect::<Vec<_>>();
        let mut rust_frame = crate::signalsmith_fft::SignalsmithStftFrame::new(block, 1440)
            .expect("native Signalsmith frame size");
        let rust = rust_frame.analysis(&input);
        let mut fixtures = crate::dsp_fixtures::FixtureReader::open();
        fixtures.skip_kaiser_windows();
        let (fft_size, native_real, native_imag) = fixtures.stft_spectrum();
        assert_eq!(fft_size, 6144);
        assert_eq!(rust.len(), fft_size / 2);
        let mut worst = 0.0f32;
        let mut worst_bin = 0;
        for bin in 0..rust.len() {
            let delta = (rust[bin].re - native_real[bin])
                .abs()
                .max((rust[bin].im - native_imag[bin]).abs());
            if delta > worst {
                worst = delta;
                worst_bin = bin;
            }
        }
        assert!(
            worst < 2.0e-2,
            "worst bin={worst_bin} max abs delta={worst}"
        );
    }

    #[test]
    fn rust_region_stretch_matches_frozen_cpp_across_stream_and_seek_transitions() {
        let mut rust = StretchState::new_seeded(17);
        assert!(rust.prepare(48_000.0, 512));
        let left = (0..8192)
            .map(|i| (i as f32 * 0.013).sin() * 0.7 + (i as f32 * 0.037).cos() * 0.2)
            .collect::<Vec<_>>();
        let right = (0..8192)
            .map(|i| (i as f32 * 0.019).sin() * 0.6)
            .collect::<Vec<_>>();
        let cases = [
            (0u64, 0.0f64, 256.0f64, 7u32, false),
            (256, 256.0, 512.0, 13, false),
            // Exercise the full accepted source-rate range. Both reference
            // engines use the same seed for deterministic random-phase cases.
            (512, 1024.0, 1088.0, 5, false),   // source rate 0.25
            (768, 1024.0, 1152.0, 3, false),   // source rate 0.5
            (1024, 1152.0, 1664.0, 21, false), // source rate 2.0
            (1280, 2048.0, 3072.0, 17, false), // source rate 4.0
            (1536, 3072.0, 3328.0, 3, false),
            (1792, 3328.0, 3584.0, 0, true),
        ];
        let mut fixtures = crate::dsp_fixtures::FixtureReader::open();
        fixtures.skip_kaiser_windows();
        fixtures.skip_stft_spectrum();
        let renders = fixtures.stretch_renders();
        assert_eq!(renders.len(), cases.len());
        for (case, (timeline, start, end, output_offset, reverse)) in cases.iter().enumerate() {
            let rust_ok = unsafe {
                rust.render(
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len() as u64,
                    0,
                    left.len() as u64,
                    *timeline,
                    *start,
                    *end,
                    *output_offset,
                    256,
                    *reverse,
                    2.0,
                    -1.0,
                    1000.0,
                )
            };
            assert!(rust_ok);
            let (reference_left, reference_right) = &renders[case];
            for i in 0..512 {
                let left_delta = (rust.output_left[i] - reference_left[i]).abs();
                let right_delta = (rust.output_right[i] - reference_right[i]).abs();
                assert!(left_delta <= 1.0e-6, "left sample {i}: delta={left_delta}");
                assert!(
                    right_delta <= 1.0e-6,
                    "right sample {i}: delta={right_delta}"
                );
            }
        }
        rust.reset();
    }
}
