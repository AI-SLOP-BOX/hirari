use std::ffi::c_void;

const MAX_FFT_SIZE: usize = 1 << 20;

pub(crate) struct FftPlan {
    size: usize,
    bit_reverse: Vec<usize>,
    twiddle_real: Vec<f32>,
    twiddle_imag: Vec<f32>,
}

impl FftPlan {
    pub(crate) fn new(size: usize) -> Option<Self> {
        if !(2..=MAX_FFT_SIZE).contains(&size) || !size.is_power_of_two() {
            return None;
        }
        let log2_size = size.trailing_zeros();
        let mut bit_reverse = Vec::with_capacity(size);
        for index in 0..size {
            let mut source = index;
            let mut reversed = 0;
            for _ in 0..log2_size {
                reversed = (reversed << 1) | (source & 1);
                source >>= 1;
            }
            bit_reverse.push(reversed);
        }
        let mut twiddle_real = Vec::with_capacity(size / 2);
        let mut twiddle_imag = Vec::with_capacity(size / 2);
        for index in 0..size / 2 {
            // Match the retired C++ FFTUtils contract: calculate the angle
            // in f64, round it to f32, then evaluate the f32 trig functions.
            let angle = (-2.0 * std::f64::consts::PI * index as f64 / size as f64) as f32;
            twiddle_real.push(angle.cos());
            twiddle_imag.push(angle.sin());
        }
        Some(Self {
            size,
            bit_reverse,
            twiddle_real,
            twiddle_imag,
        })
    }

    pub(crate) fn forward(&self, real: &mut [f32], imag: &mut [f32]) {
        if real.len() < self.size || imag.len() < self.size {
            return;
        }
        for index in 0..self.size {
            let reverse = self.bit_reverse[index];
            if index < reverse {
                real.swap(index, reverse);
                imag.swap(index, reverse);
            }
        }
        let mut span = 2;
        while span <= self.size {
            let half = span / 2;
            for block_start in (0..self.size).step_by(span) {
                for offset in 0..half {
                    let twiddle = offset * (self.size / span);
                    let i1 = block_start + offset;
                    let i2 = i1 + half;
                    let tr = self.twiddle_real[twiddle] * real[i2]
                        - self.twiddle_imag[twiddle] * imag[i2];
                    let ti = self.twiddle_real[twiddle] * imag[i2]
                        + self.twiddle_imag[twiddle] * real[i2];
                    real[i2] = real[i1] - tr;
                    imag[i2] = imag[i1] - ti;
                    real[i1] += tr;
                    imag[i1] += ti;
                }
            }
            span <<= 1;
        }
    }

    pub(crate) fn inverse(&self, real: &mut [f32], imag: &mut [f32]) {
        if real.len() < self.size || imag.len() < self.size {
            return;
        }
        for value in &mut imag[..self.size] {
            *value = -*value;
        }
        self.forward(real, imag);
        let scale = 1.0 / self.size as f32;
        for index in 0..self.size {
            real[index] *= scale;
            imag[index] *= -scale;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_fft_plan_create(size: usize) -> *mut c_void {
    FftPlan::new(size).map_or(std::ptr::null_mut(), |plan| {
        Box::into_raw(Box::new(plan)).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fft_plan_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<FftPlan>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fft_plan_valid(state: *const c_void) -> bool {
    !state.is_null()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fft_forward(state: *const c_void, real: *mut f32, imag: *mut f32) {
    if state.is_null() || real.is_null() || imag.is_null() {
        return;
    }
    let plan = &*state.cast::<FftPlan>();
    plan.forward(
        std::slice::from_raw_parts_mut(real, plan.size),
        std::slice::from_raw_parts_mut(imag, plan.size),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fft_inverse(state: *const c_void, real: *mut f32, imag: *mut f32) {
    if state.is_null() || real.is_null() || imag.is_null() {
        return;
    }
    let plan = &*state.cast::<FftPlan>();
    plan.inverse(
        std::slice::from_raw_parts_mut(real, plan.size),
        std::slice::from_raw_parts_mut(imag, plan.size),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_profile_analyze(
    input: *const f32,
    size: u32,
    magnitude_output: *mut f32,
) -> bool {
    let size = size as usize;
    if input.is_null()
        || magnitude_output.is_null()
        || !(2..=MAX_FFT_SIZE).contains(&size)
        || !size.is_power_of_two()
    {
        return false;
    }

    let mut real = Vec::new();
    let mut imag = Vec::new();
    if real.try_reserve_exact(size).is_err() || imag.try_reserve_exact(size).is_err() {
        return false;
    }
    real.resize(size, 0.0_f32);
    imag.resize(size, 0.0_f32);

    let input = std::slice::from_raw_parts(input, size);
    for index in 0..size {
        let window =
            0.5_f64 * (1.0 - (2.0 * std::f64::consts::PI * index as f64 / (size - 1) as f64).cos());
        real[index] = input[index] * window as f32;
    }
    let Some(plan) = FftPlan::new(size) else {
        return false;
    };
    plan.forward(&mut real, &mut imag);

    let output = std::slice::from_raw_parts_mut(magnitude_output, size / 2);
    for index in 0..size / 2 {
        output[index] = (real[index] * real[index] + imag[index] * imag[index]).sqrt();
    }
    true
}
