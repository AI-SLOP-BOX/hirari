use std::ptr;

unsafe extern "C" {
    fn hirari_stretch_kernel_create() -> *mut std::ffi::c_void;
    fn hirari_stretch_kernel_create_seeded(seed: std::ffi::c_long) -> *mut std::ffi::c_void;
    fn hirari_stretch_kernel_destroy(handle: *mut std::ffi::c_void);
    fn hirari_stretch_kernel_prepare(handle: *mut std::ffi::c_void, sample_rate: f32) -> bool;
    fn hirari_stretch_kernel_reset(handle: *mut std::ffi::c_void);
    fn hirari_stretch_kernel_seek_length(handle: *mut std::ffi::c_void, rate: f32) -> i32;
    fn hirari_stretch_kernel_latency(handle: *mut std::ffi::c_void) -> i32;
    fn hirari_stretch_kernel_set_controls(
        handle: *mut std::ffi::c_void,
        transpose: f32,
        formant: f32,
        formant_base: f32,
    ) -> bool;
    fn hirari_stretch_kernel_seek(
        handle: *mut std::ffi::c_void,
        left: *const f32,
        right: *const f32,
        samples: i32,
    ) -> bool;
    fn hirari_stretch_kernel_process(
        handle: *mut std::ffi::c_void,
        input_left: *const f32,
        input_right: *const f32,
        input_samples: i32,
        output_left: *mut f32,
        output_right: *mut f32,
        output_samples: i32,
    ) -> bool;
}

struct StretchState {
    kernel: *mut std::ffi::c_void,
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
        Self::with_kernel(unsafe { hirari_stretch_kernel_create() })
    }

    fn with_kernel(kernel: *mut std::ffi::c_void) -> Self {
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

    #[cfg(all(test, feature = "dsp-differential-reference"))]
    fn new_seeded(seed: std::ffi::c_long) -> Self {
        Self::with_kernel(unsafe { hirari_stretch_kernel_create_seeded(seed) })
    }

    fn reset(&mut self) {
        if !self.kernel.is_null() {
            unsafe { hirari_stretch_kernel_reset(self.kernel) };
        }
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
        if self.kernel.is_null()
            || !unsafe { hirari_stretch_kernel_prepare(self.kernel, sample_rate as f32) }
        {
            return false;
        }
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
        let seek_len = unsafe {
            hirari_stretch_kernel_seek_length(self.kernel, 4.0)
                .max(hirari_stretch_kernel_seek_length(self.kernel, 0.25))
        };
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
        if !unsafe {
            hirari_stretch_kernel_set_controls(self.kernel, transpose, formant, formant_base)
        } {
            return false;
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
        if !unsafe {
            hirari_stretch_kernel_process(
                self.kernel,
                self.input_left.as_ptr(),
                self.input_right.as_ptr(),
                input_count as i32,
                self.output_left.as_mut_ptr().add(out_start),
                self.output_right.as_mut_ptr().add(out_start),
                frames as i32,
            )
        } {
            return false;
        }
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
        let seek_samples =
            unsafe { hirari_stretch_kernel_seek_length(self.kernel, source_rate as f32) };
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
        if !unsafe {
            hirari_stretch_kernel_seek(
                self.kernel,
                self.seek_left.as_ptr(),
                self.seek_right.as_ptr(),
                seek_samples,
            )
        } {
            return false;
        }
        self.next_input_offset = input_position;
        self.stream_valid = true;
        true
    }
}

impl Drop for StretchState {
    fn drop(&mut self) {
        if !self.kernel.is_null() {
            unsafe { hirari_stretch_kernel_destroy(self.kernel) };
            self.kernel = ptr::null_mut();
        }
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
    unsafe { hirari_stretch_kernel_latency(state.kernel).max(0) as u32 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_stretch_reset(handle: *mut std::ffi::c_void) {
    if let Some(state) = unsafe { handle.cast::<StretchState>().as_mut() } {
        state.reset();
    }
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

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod differential_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_signalsmith_stft_analyse_reference(
            input: *const f32,
            block_size: usize,
            interval: usize,
            spectrum_real: *mut f32,
            spectrum_imag: *mut f32,
            spectrum_capacity: usize,
            fft_size: *mut usize,
        ) -> bool;
        fn hirari_region_stretch_reference_create() -> *mut std::ffi::c_void;
        fn hirari_region_stretch_reference_destroy(handle: *mut std::ffi::c_void);
        fn hirari_region_stretch_reference_prepare(
            handle: *mut std::ffi::c_void,
            sample_rate: f64,
            max_block_size: u32,
        ) -> bool;
        fn hirari_region_stretch_reference_render(
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
        ) -> bool;
        fn hirari_region_stretch_reference_left(handle: *const std::ffi::c_void) -> *const f32;
        fn hirari_region_stretch_reference_right(handle: *const std::ffi::c_void) -> *const f32;
        fn hirari_region_stretch_reference_reset(handle: *mut std::ffi::c_void);
    }

    #[test]
    fn rust_half_bin_stft_analysis_matches_signalsmith_native_reference() {
        let block = 5760;
        let interval = 1440;
        let input = (0..block)
            .map(|index| {
                let x = index as f32;
                (x * 0.013).sin() * 0.6 + (x * 0.0017).cos() * 0.2
            })
            .collect::<Vec<_>>();
        let mut rust_frame = crate::signalsmith_fft::SignalsmithStftFrame::new(block, interval)
            .expect("native Signalsmith frame size");
        let rust = rust_frame.analysis(&input);
        let mut native_real = vec![0.0f32; rust.len()];
        let mut native_imag = vec![0.0f32; rust.len()];
        let mut native_fft_size = 0usize;
        assert!(unsafe {
            hirari_signalsmith_stft_analyse_reference(
                input.as_ptr(),
                block,
                interval,
                native_real.as_mut_ptr(),
                native_imag.as_mut_ptr(),
                native_real.len(),
                &mut native_fft_size,
            )
        });
        assert_eq!(native_fft_size, 6144);
        assert_eq!(rust.len(), native_fft_size / 2);
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
        let reference = unsafe { hirari_region_stretch_reference_create() };
        assert!(!reference.is_null());
        assert!(rust.prepare(48_000.0, 512));
        assert!(unsafe { hirari_region_stretch_reference_prepare(reference, 48_000.0, 512) });
        let left = (0..8192)
            .map(|i| (i as f32 * 0.013).sin() * 0.7 + (i as f32 * 0.037).cos() * 0.2)
            .collect::<Vec<_>>();
        let right = (0..8192)
            .map(|i| (i as f32 * 0.019).sin() * 0.6)
            .collect::<Vec<_>>();
        let cases = [
            (0, 0.0, 256.0, 7, false),
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
        for (timeline, start, end, output_offset, reverse) in cases {
            let rust_ok = unsafe {
                rust.render(
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len() as u64,
                    0,
                    left.len() as u64,
                    timeline,
                    start,
                    end,
                    output_offset,
                    256,
                    reverse,
                    2.0,
                    -1.0,
                    1000.0,
                )
            };
            let reference_ok = unsafe {
                hirari_region_stretch_reference_render(
                    reference,
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len() as u64,
                    0,
                    left.len() as u64,
                    timeline,
                    start,
                    end,
                    output_offset,
                    256,
                    reverse,
                    2.0,
                    -1.0,
                    1000.0,
                )
            };
            assert_eq!(rust_ok, reference_ok);
            assert!(rust_ok);
            let reference_left = unsafe { hirari_region_stretch_reference_left(reference) };
            let reference_right = unsafe { hirari_region_stretch_reference_right(reference) };
            for i in 0..512 {
                let left_delta = (rust.output_left[i] - unsafe { *reference_left.add(i) }).abs();
                let right_delta = (rust.output_right[i] - unsafe { *reference_right.add(i) }).abs();
                assert!(left_delta <= 1.0e-6, "left sample {i}: delta={left_delta}");
                assert!(
                    right_delta <= 1.0e-6,
                    "right sample {i}: delta={right_delta}"
                );
            }
        }
        rust.reset();
        unsafe {
            hirari_region_stretch_reference_reset(reference);
            hirari_region_stretch_reference_destroy(reference);
        }
    }
}
