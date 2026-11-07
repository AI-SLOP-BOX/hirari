//! Frozen pre-Rust masking analyser with its legacy radix-2 transform.

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

fn legacy_forward(values: &mut [Complex]) {
    let size = values.len();
    let mut j = 0usize;
    for i in 1..size {
        let mut bit = size >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut length = 2usize;
    while length <= size {
        let step = size / length;
        for start in (0..size).step_by(length) {
            for offset in 0..length / 2 {
                let lut_index = (512 - step * offset) % 512;
                let angle = 2.0 * std::f64::consts::PI * lut_index as f64 / 512.0;
                let twiddle_re = angle.cos() as f32;
                let twiddle_im = -(angle.sin()) as f32;
                let even = values[start + offset];
                let odd = values[start + offset + length / 2];
                let odd_re = odd.re * twiddle_re - odd.im * twiddle_im;
                let odd_im = odd.re * twiddle_im + odd.im * twiddle_re;
                values[start + offset] = Complex {
                    re: even.re + odd_re,
                    im: even.im + odd_im,
                };
                values[start + offset + length / 2] = Complex {
                    re: even.re - odd_re,
                    im: even.im - odd_im,
                };
            }
        }
        length <<= 1;
    }
}

fn magnitude(value: Complex) -> f32 {
    (value.re * value.re + value.im * value.im).sqrt()
}

