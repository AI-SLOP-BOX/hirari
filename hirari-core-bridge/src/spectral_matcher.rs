use std::ffi::c_void;

const FFT_SIZE: usize = 4096;
const BIN_COUNT: usize = FFT_SIZE / 2;
const FLOOR: f32 = 1.0e-12;
const AVERAGE_ALPHA: f32 = 0.01;

struct SpectralMatcher {
    target: [f32; BIN_COUNT],
    average: [f32; BIN_COUNT],
}

impl SpectralMatcher {
    fn new() -> Self {
        Self {
            target: [0.001; BIN_COUNT],
            average: [0.001; BIN_COUNT],
        }
    }

    fn set_reference(&mut self, target: &[f32]) {
        if target.len() < BIN_COUNT {
            return;
        }
        for (current, &value) in self.target.iter_mut().zip(target) {
            if value.is_finite() && value >= 0.0 {
                *current = value.max(FLOOR);
            }
        }
    }

    fn update_average(&mut self, input: &[f32]) {
        for (average, &magnitude) in self.average.iter_mut().zip(input) {
            if magnitude.is_finite() {
                *average += AVERAGE_ALPHA * (magnitude.max(FLOOR) - *average);
            }
        }
    }

    fn calculate_match_curve(&self, output: &mut [f32]) {
        for ((&average, &target), result) in self.average.iter().zip(&self.target).zip(output) {
            let input_db = 20.0 * average.max(FLOOR).log10();
            let target_db = 20.0 * target.max(FLOOR).log10();
            *result = (target_db - input_db).clamp(-12.0, 12.0);
        }
    }

    fn set_pink_noise_reference(&mut self) {
        self.target[0] = 1.0;
        for index in 1..BIN_COUNT {
            self.target[index] = 1.0 / (index as f32).sqrt();
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_spectral_matcher_create() -> *mut c_void {
    Box::into_raw(Box::new(SpectralMatcher::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_matcher_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SpectralMatcher>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_matcher_set_reference(
    state: *mut c_void,
    values: *const f32,
    length: usize,
) {
    if state.is_null() || values.is_null() {
        return;
    }
    (*state.cast::<SpectralMatcher>()).set_reference(std::slice::from_raw_parts(values, length));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_matcher_update_average(
    state: *mut c_void,
    values: *const f32,
    length: usize,
) {
    if state.is_null() || values.is_null() {
        return;
    }
    (*state.cast::<SpectralMatcher>()).update_average(std::slice::from_raw_parts(values, length));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_matcher_calculate(
    state: *const c_void,
    output: *mut f32,
    capacity: usize,
) -> bool {
    if state.is_null() || output.is_null() || capacity < BIN_COUNT {
        return false;
    }
    (*state.cast::<SpectralMatcher>())
        .calculate_match_curve(std::slice::from_raw_parts_mut(output, BIN_COUNT));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_matcher_set_pink_noise_reference(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<SpectralMatcher>()).set_pink_noise_reference();
    }
}
