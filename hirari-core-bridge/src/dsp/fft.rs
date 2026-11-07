
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

